//! The states of every pane as scenes, `<slug>/<state>`, made from one table of panes and four
//! statuses: each puts the pane's sources and Yi's link in that status through `step`.

use yd_core::command::table::{OPEN_FILE, Route, Run, Target};
use yd_core::feed::{
    ASSETS, CHANGES, CONTAINERS, DAEMONS, DEBUGGER, FILES, FeedEvent, FeedStatus, LSP, REMOTE,
    REPORT, VMS, WEB,
};
use yd_core::ids::{FeedId, Millis};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::wire::event::{Down, WireEvent};

use crate::pane::{SCENE_PANE, Scene, Stage};
use crate::term::view::state::Shells;
use crate::win::Hub;

const LOGS: FeedId = FeedId::new("logs");
const PLUGINS: FeedId = FeedId::new("plugins");

/// One pane: the sources its view reads, whether it needs Yi's server, and what its source
/// says when it fails, in that source's own words.
struct Pane {
    kind: PaneKind,
    feeds: fn(&mut Hub, Shown) -> Vec<FeedId>,
    server: bool,
    failed: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Loading,
    Empty,
    Offline,
    Error,
}

const SHOWN: [Shown; 4] = [Shown::Empty, Shown::Loading, Shown::Offline, Shown::Error];

fn none(_: &mut Hub, _: Shown) -> Vec<FeedId> {
    Vec::new()
}

macro_rules! of {
    ($name:ident, $($feed:expr),+) => {
        fn $name(_: &mut Hub, _: Shown) -> Vec<FeedId> {
            vec![$($feed),+]
        }
    };
}

of!(files, FILES, CHANGES);
of!(changes, CHANGES);
of!(web, WEB);
of!(report, REPORT);
of!(logs, LOGS);
of!(assets, ASSETS);
of!(containers, CONTAINERS);
of!(vms, VMS);
of!(daemons, DAEMONS);
of!(debugger, DEBUGGER);
of!(lsp, LSP);
of!(plugins, PLUGINS);
of!(remote, REMOTE);

/// The Design pane reads the page's feed: with no page asked for it is empty, so loading is
/// a page that was asked for and has not come.
fn design(hub: &mut Hub, shown: Shown) -> Vec<FeedId> {
    if shown == Shown::Loading {
        let loading = FeedEvent::Status(FeedStatus::Loading);
        step(&mut hub.model, Msg::Feed(WEB, loading));
    }
    vec![WEB]
}

/// The code viewer is empty with no file open; otherwise a file is open and its text is what
/// the files source has not sent.
fn code(hub: &mut Hub, shown: Shown) -> Vec<FeedId> {
    if shown == Shown::Empty {
        return Vec::new();
    }
    if let Ok(file) = "yi://file/src/lib.rs".parse() {
        let run = Run::new(OPEN_FILE).on(Target::Thing(file));
        step(&mut hub.model, Msg::Command(Route::Model, run));
    }
    vec![FILES]
}

/// The terminal's first shell, made as the pane would make it.
fn terminal(hub: &mut Hub, _: Shown) -> Vec<FeedId> {
    let shells = hub.panes.get_mut::<Shells>(Some(SCENE_PANE));
    shells
        .map(|shells| shells.tabs.iter().map(|shell| shell.feed).collect())
        .unwrap_or_default()
}

const fn pane(
    kind: PaneKind,
    feeds: fn(&mut Hub, Shown) -> Vec<FeedId>,
    failed: &'static str,
) -> Pane {
    Pane {
        kind,
        feeds,
        server: false,
        failed,
    }
}

const fn server(kind: PaneKind) -> Pane {
    Pane {
        kind,
        feeds: none,
        server: true,
        failed: "",
    }
}

const PANES: [Pane; 24] = [
    server(PaneKind::Chat),
    pane(PaneKind::Files, files, "Permission denied (os error 13)"),
    pane(
        PaneKind::Changes,
        changes,
        "fatal: not a git repository (or any of the parent directories): .git",
    ),
    pane(
        PaneKind::Code,
        code,
        "No such file or directory (os error 2)",
    ),
    pane(
        PaneKind::Terminal,
        terminal,
        "yi __pty: No such file or directory",
    ),
    pane(PaneKind::Web, web, "Chrome exited before its first frame"),
    Pane {
        server: true,
        ..pane(
            PaneKind::Report,
            report,
            "Chrome exited before its first frame",
        )
    },
    server(PaneKind::Agents),
    server(PaneKind::Trace),
    pane(
        PaneKind::Logs,
        logs,
        "the worker log could not be opened: Permission denied",
    ),
    server(PaneKind::Findings),
    pane(
        PaneKind::Assets,
        assets,
        "no machines adapter is configured",
    ),
    pane(
        PaneKind::Containers,
        containers,
        "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?",
    ),
    pane(PaneKind::Vms, vms, "the VM host did not answer within 10 s"),
    pane(
        PaneKind::Daemons,
        daemons,
        "launchctl list exited with status 1",
    ),
    pane(
        PaneKind::Debugger,
        debugger,
        "lldb-dap: No such file or directory",
    ),
    pane(
        PaneKind::LanguageServers,
        lsp,
        "rust-analyzer exited with status 101",
    ),
    pane(
        PaneKind::Plugins,
        plugins,
        "panes/queue.luau:12: attempt to index nil with 'state'",
    ),
    server(PaneKind::Settings),
    server(PaneKind::PluginKit),
    pane(
        PaneKind::Script,
        plugins,
        "panes/queue.luau:12: attempt to index nil with 'state'",
    ),
    pane(
        PaneKind::Design,
        design,
        "Chrome exited before its first frame",
    ),
    pane(
        PaneKind::Remote,
        remote,
        "the machine host did not answer within 10 s",
    ),
    server(PaneKind::IPython),
];

/// Puts pane `P`'s sources and the link in status `S` of [`SHOWN`].
fn setup<const P: usize, const S: usize>(hub: &mut Hub) {
    let (Some(pane), Some(shown)) = (PANES.get(P), SHOWN.get(S).copied()) else {
        return;
    };
    let at = Millis(0);
    let status = match shown {
        Shown::Loading => None,
        Shown::Empty => Some(FeedStatus::Live),
        Shown::Offline => Some(FeedStatus::Offline),
        Shown::Error => Some(FeedStatus::Error(pane.failed.to_owned())),
    };
    for feed in (pane.feeds)(hub, shown) {
        if let Some(status) = status.clone() {
            step(
                &mut hub.model,
                Msg::Feed(feed, FeedEvent::Reset(Vec::new())),
            );
            step(&mut hub.model, Msg::Feed(feed, FeedEvent::Status(status)));
        }
    }
    if !pane.server {
        return;
    }
    let link = match shown {
        Shown::Loading => None,
        Shown::Empty => Some(WireEvent::Up {
            at,
            server: "0.300.0".to_owned(),
            latency: None,
        }),
        Shown::Offline => Some(WireEvent::Down {
            at,
            why: Down::NoServer,
        }),
        Shown::Error => Some(WireEvent::Down {
            at,
            why: Down::StartFailed("yi: command not found".to_owned()),
        }),
    };
    if let Some(link) = link {
        step(&mut hub.model, Msg::Wire(link));
    }
}

macro_rules! at {
    (empty) => {
        0
    };
    (loading) => {
        1
    };
    (offline) => {
        2
    };
    (error) => {
        3
    };
}

/// A scene for each named state of each row; a pane whose own scene already shows a state
/// leaves it out here.
macro_rules! states {
    ($($row:literal $slug:literal [$($state:ident)+]),+ $(,)?) => {
        &[$($(
            Scene {
                name: concat!($slug, "/", stringify!($state)),
                stage: Stage::Pane(PANES[$row].kind),
                setup: setup::<$row, { at!($state) }>,
            },
        )+)+]
    };
}

#[rustfmt::skip]
pub const SCENES: &[Scene] = states![
    0 "chat" [loading offline error],
    1 "files" [loading offline error],
    2 "changes" [loading offline error],
    3 "code" [loading offline error],
    4 "terminal" [empty loading offline error],
    5 "web" [empty loading offline error],
    6 "report" [loading offline error],
    7 "agents" [empty loading offline error],
    8 "trace" [empty loading offline error],
    9 "logs" [loading offline error],
    10 "findings" [empty loading offline error],
    11 "assets" [empty loading offline error],
    12 "containers" [empty loading offline error],
    13 "vms" [empty loading offline error],
    14 "daemons" [empty loading offline error],
    15 "debugger" [empty loading offline error],
    16 "lsp" [empty loading offline error],
    17 "plugins" [empty loading offline error],
    18 "settings" [empty loading offline error],
    19 "plugin-kit" [empty loading offline error],
    20 "script" [empty loading offline],
    21 "design" [loading offline error],
    22 "remote" [empty loading offline error],
    23 "ipython" [empty loading offline error],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_draws_the_pane_its_name_says() -> Result<(), Box<dyn std::error::Error>> {
        for scene in SCENES {
            let slug = scene.name.split('/').next();
            let Stage::Pane(kind) = scene.stage else {
                return Err(format!("{} is not a pane's scene", scene.name).into());
            };
            assert_eq!(slug, Some(kind.slug()), "{}", scene.name);
        }
        Ok(())
    }
}
