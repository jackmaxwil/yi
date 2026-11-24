//! One machines adapter behind the VMs, Assets and Remote panes: recorded data, or none. A real
//! host is one more [`Host`]; none is named yet, so recorded replays the mockup and refuses commands.

use std::sync::mpsc::Receiver;

use yd_core::command::table::{Run, Target};
use yd_core::command::tools::remote::{
    self, NO_HOST, RECORDED_MACHINE, RECORDED_ONLY, Sample, sample,
};
use yd_core::feed::{FeedEvent, FeedStatus, SourceCmd};
use yd_core::inbox::{Closed, FeedTx};
use yd_core::row::{Row, rows_from_json};

pub use yd_core::feed::{ASSETS, REMOTE, VMS};

const VM_ROWS: &str = include_str!("../../../fixtures/mockup/vms.rows.json");
const ASSET_ROWS: &str = include_str!("../../../fixtures/mockup/assets.rows.json");

/// Which adapter answers, as the setting `machines.adapter` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Adapter {
    #[default]
    None,
    Recorded,
}

impl Adapter {
    /// The adapter a setting value names; any other name is no adapter.
    pub fn named(name: &str) -> Self {
        match name {
            "recorded" => Self::Recorded,
            _ => Self::None,
        }
    }

    fn host(self) -> Option<&'static dyn Host> {
        match self {
            Self::None => None,
            Self::Recorded => Some(&Recorded),
        }
    }
}

/// What a machine host must answer: its machines, its assets, and a machine's screen.
pub trait Host {
    fn machines(&self) -> Result<Vec<Row>, String>;
    fn assets(&self) -> Result<Vec<Row>, String>;
    fn screen(&self, machine: &str) -> Result<Sample, String>;
}

struct Recorded;

impl Host for Recorded {
    fn machines(&self) -> Result<Vec<Row>, String> {
        rows_from_json(VM_ROWS).map_err(|why| why.to_string())
    }

    fn assets(&self) -> Result<Vec<Row>, String> {
        rows_from_json(ASSET_ROWS).map_err(|why| why.to_string())
    }

    fn screen(&self, machine: &str) -> Result<Sample, String> {
        if machine != RECORDED_MACHINE {
            return Err(format!("{machine} has no recorded screen"));
        }
        sample()
    }
}

fn refused(adapter: Adapter) -> &'static str {
    match adapter {
        Adapter::None => NO_HOST,
        Adapter::Recorded => RECORDED_ONLY,
    }
}

fn status(out: &FeedTx, status: FeedStatus) -> Result<(), Closed> {
    out.send(FeedEvent::Status(status))
}

/// Which list a source serves.
#[derive(Clone, Copy)]
enum List {
    Machines,
    Assets,
}

/// Sends the list's rows, or says why there are none.
fn publish(out: &FeedTx, adapter: Adapter, list: List) -> Result<(), Closed> {
    let Some(host) = adapter.host() else {
        return status(out, FeedStatus::Error(NO_HOST.to_owned()));
    };
    let read = match list {
        List::Machines => host.machines(),
        List::Assets => host.assets(),
    };
    match read {
        Ok(rows) => {
            out.send(FeedEvent::Reset(rows))?;
            status(out, FeedStatus::Live)
        }
        Err(why) => status(out, FeedStatus::Error(why)),
    }
}

/// The adapter a `remote.adapter` ask names.
fn named(run: &Run) -> Option<Adapter> {
    (run.command == remote::ADAPTER).then(|| Adapter::named(run.text.as_deref().unwrap_or("")))
}

/// Serves one list: rows while a pane shows it, a refusal for any command.
fn serve_list(out: FeedTx, cmds: Receiver<SourceCmd>, list: List) -> Result<(), Closed> {
    let (mut adapter, mut seen) = (Adapter::None, false);
    while let Ok(ask) = cmds.recv() {
        match ask {
            SourceCmd::Run(run) => match named(&run) {
                Some(now) => {
                    adapter = now;
                    if seen {
                        publish(&out, adapter, list)?;
                    }
                }
                None => status(&out, FeedStatus::Error(refused(adapter).to_owned()))?,
            },
            SourceCmd::Seen(now) => {
                seen = now;
                if now {
                    publish(&out, adapter, list)?;
                }
            }
            SourceCmd::Refresh if seen => publish(&out, adapter, list)?,
            _ => {}
        }
    }
    Ok(())
}

/// The machines of the host, as the VMs pane lists them.
pub fn vms(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    serve_list(out, cmds, List::Machines)
}

/// The hosts and other assets, as the Assets pane lists them.
pub fn assets(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    serve_list(out, cmds, List::Assets)
}

/// Where a picture goes: the pane's screen, which says why when it refuses one.
pub type Show<'a> = &'a dyn Fn(u32, u32, Vec<u8>) -> Result<(), String>;

/// The machine a `remote.open` names: its `vm` address, none for the first.
fn opened(run: &Run) -> Option<String> {
    match &run.target {
        Target::Thing(address) => Some(address.id().to_owned()),
        Target::App | Target::Pane(_) => None,
    }
}

/// Serves one machine's screen: its rows and picture once opened, and the refusal of any input.
pub fn remote_screen(out: FeedTx, cmds: Receiver<SourceCmd>, show: Show<'_>) -> Result<(), Closed> {
    let (mut adapter, mut open): (Option<Adapter>, Option<Option<String>>) = (None, None);
    let mut screen: Option<Row> = None;
    let mut told = false;
    while let Ok(ask) = cmds.recv() {
        match ask {
            SourceCmd::Run(run) if run.command == remote::OPEN => open = Some(opened(&run)),
            SourceCmd::Run(run) => match named(&run) {
                Some(now) => adapter = Some(now),
                None => continue,
            },
            SourceCmd::Screen(_) => {
                if let Some(row) = screen.as_ref().filter(|_| !told) {
                    out.send(FeedEvent::Upsert(row.clone().text("note", RECORDED_ONLY)))?;
                }
                told = true;
                continue;
            }
            _ => continue,
        }
        if let (Some(adapter), Some(machine)) = (adapter, &open) {
            let machine = machine.as_deref().unwrap_or(RECORDED_MACHINE);
            screen = look(&out, adapter, machine, show)?;
            told = false;
        }
    }
    Ok(())
}

/// Sends the machine's screen, or says why not; returns the `screen:0` row as sent.
fn look(
    out: &FeedTx,
    adapter: Adapter,
    machine: &str,
    show: Show<'_>,
) -> Result<Option<Row>, Closed> {
    let Some(host) = adapter.host() else {
        status(out, FeedStatus::Error(NO_HOST.to_owned()))?;
        return Ok(None);
    };
    let seen = match host.screen(machine) {
        Ok(seen) => seen,
        Err(why) => {
            status(out, FeedStatus::Error(why))?;
            return Ok(None);
        }
    };
    let shown = show(seen.width, seen.height, seen.rgba);
    let mut rows = seen.rows;
    let refused = shown
        .err()
        .map(|why| format!("The picture was refused: {why}"));
    if let (Some(note), Some(first)) = (refused, rows.first_mut()) {
        *first = first.clone().text("note", note);
    }
    let screen = rows.first().cloned();
    out.send(FeedEvent::Reset(rows))?;
    status(out, FeedStatus::Live)?;
    Ok(screen)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::mpsc::channel;

    use yd_core::address::uri::Address;
    use yd_core::command::table::Run;
    use yd_core::feed::ScreenInput;
    use yd_core::feed::{SourceFn, Table};
    use yd_core::ids::FeedId;
    use yd_core::ids::RowId;
    use yd_core::inbox::inbox;
    use yd_core::kind::Kind;
    use yd_core::model::Model;
    use yd_core::row::{Rows, rows_from_json};

    use super::*;

    type Check = Result<(), Box<dyn Error>>;

    const VM_ROWS: &str = include_str!("../../../fixtures/mockup/vms.rows.json");
    const ASSET_ROWS: &str = include_str!("../../../fixtures/mockup/assets.rows.json");

    fn adapter(name: &str) -> SourceCmd {
        let run = Run::new(remote::ADAPTER);
        SourceCmd::Run(Run {
            text: Some(name.to_owned()),
            ..run
        })
    }

    /// Runs `source` over `asks`, then returns the table it filled, with the version it had
    /// before the last ask.
    fn fed(feed: FeedId, source: SourceFn, asks: Vec<SourceCmd>) -> (Table, u64) {
        let (tx, inbox) = inbox(|| {});
        let (cmd_tx, cmd_rx) = channel();
        let before = asks.len().saturating_sub(1);
        let mut model = Model::default();
        let mut version = 0;
        for (n, ask) in asks.into_iter().enumerate() {
            cmd_tx.send(ask).ok();
            if n == before {
                inbox.drain(&mut model);
                version = model.tables.get(feed).map_or(0, Table::version);
            }
        }
        drop(cmd_tx);
        let _ = source(tx.feed(feed), cmd_rx);
        inbox.drain(&mut model);
        (model.tables.get(feed).cloned().unwrap_or_default(), version)
    }

    #[test]
    fn with_no_adapter_both_feeds_say_no_host_is_set_up() {
        for (feed, source) in [(VMS, vms as SourceFn), (ASSETS, assets as SourceFn)] {
            let (table, _) = fed(feed, source, vec![SourceCmd::Seen(true)]);
            assert_eq!(table.status(), &FeedStatus::Error(NO_HOST.to_owned()));
            assert_eq!(table.len(), 0);
        }
    }

    #[test]
    fn recorded_rows_carry_the_fixtures_field_names() -> Check {
        let asks = vec![adapter("recorded"), SourceCmd::Seen(true)];
        for (feed, source, fixture) in [
            (VMS, vms as SourceFn, VM_ROWS),
            (ASSETS, assets as SourceFn, ASSET_ROWS),
        ] {
            let (table, _) = fed(feed, source, asks.clone());
            let want = rows_from_json(fixture)?;
            assert_eq!(table.status(), &FeedStatus::Live);
            assert_eq!(table.len(), want.len());
            for (at, row) in want.iter().enumerate() {
                let got = table.get(at).ok_or("a row is missing")?;
                let names = |r: &yd_core::row::Row| r.cells.keys().cloned().collect::<Vec<_>>();
                assert_eq!((&got.id, names(got)), (&row.id, names(row)));
            }
        }
        Ok(())
    }

    #[test]
    fn a_vm_command_on_recorded_data_changes_nothing_and_says_so() {
        let stop = SourceCmd::Run(Run::new(yd_core::ids::CommandId("vm.stop")));
        let seen = vec![adapter("recorded"), SourceCmd::Seen(true)];
        let (read, _) = fed(VMS, vms, seen.clone());
        let (after, _) = fed(VMS, vms, [seen, vec![stop]].concat());
        assert_eq!(after.status(), &FeedStatus::Error(RECORDED_ONLY.to_owned()));
        let rows = |t: &Table| {
            (0..t.len())
                .filter_map(|i| t.get(i).cloned())
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(&after), rows(&read), "no row moved");
    }

    /// Runs the screen source over `asks`; returns its table and the pictures it showed.
    fn screened(asks: Vec<SourceCmd>, refuse: bool) -> (Table, Vec<(u32, u32, usize)>) {
        let (tx, inbox) = inbox(|| {});
        let (cmd_tx, cmd_rx) = channel();
        for ask in asks {
            cmd_tx.send(ask).ok();
        }
        drop(cmd_tx);
        let shown = std::cell::RefCell::new(Vec::new());
        let show = |w: u32, h: u32, rgba: Vec<u8>| {
            shown.borrow_mut().push((w, h, rgba.len()));
            if refuse {
                Err("too big".to_owned())
            } else {
                Ok(())
            }
        };
        let _ = remote_screen(tx.feed(REMOTE), cmd_rx, &show);
        let mut model = Model::default();
        inbox.drain(&mut model);
        let table = model.tables.get(REMOTE).cloned().unwrap_or_default();
        (table, shown.into_inner())
    }

    fn open(machine: Option<&str>) -> SourceCmd {
        let run = Run::new(remote::OPEN);
        let target = machine.and_then(|id| Address::new(Kind::Vm, id).ok());
        SourceCmd::Run(match target {
            Some(address) => run.on(Target::Thing(address)),
            None => run,
        })
    }

    fn cell(table: &Table, row: &str, field: &str) -> Option<String> {
        let found = table.find(&RowId::new(row))?;
        found.words(field).map(str::to_owned)
    }

    #[test]
    fn with_no_adapter_the_screen_says_no_host_is_set_up() {
        let (table, shown) = screened(vec![adapter("none"), open(None)], false);
        assert_eq!(table.status(), &FeedStatus::Error(NO_HOST.to_owned()));
        assert!(shown.is_empty(), "no picture without a host");
    }

    #[test]
    fn the_recorded_screen_has_the_record_s_windows_rows_and_one_picture() {
        let (table, shown) = screened(vec![open(None), adapter("recorded")], false);
        assert_eq!(table.status(), &FeedStatus::Live);
        assert_eq!(shown, [(860, 484, 860 * 484 * 4)]);
        assert_eq!(
            cell(&table, "screen:0", "machine").as_deref(),
            Some("ci-ubuntu")
        );
        assert_eq!(
            cell(&table, "screen:0", "size").as_deref(),
            Some("1920 × 1080")
        );
        let uploads = [
            ("item:hero-01", "hero-01.png", "Uploaded"),
            ("item:hero-02", "hero-02.png", "Uploaded"),
            ("item:detail-front", "detail-front.png", "Retrying in 41 s"),
            ("item:detail-back", "detail-back.png", "Waiting"),
        ];
        for (id, name, status) in uploads {
            assert_eq!(cell(&table, id, "name").as_deref(), Some(name), "{id}");
            assert_eq!(cell(&table, id, "status").as_deref(), Some(status), "{id}");
        }
        assert_eq!(
            cell(&table, "window:atlas-uploader", "name").as_deref(),
            Some("Atlas Uploader")
        );
        let last = cell(&table, "line:5", "name");
        assert_eq!(
            last.as_deref(),
            Some("17:58:03 will retry detail-front.png")
        );
    }

    #[test]
    fn input_on_the_recorded_screen_is_refused_in_the_row_the_bar_reads() {
        let press = SourceCmd::Screen(ScreenInput::Text("x".to_owned()));
        let asks = vec![adapter("recorded"), open(None), press];
        let (table, _) = screened(asks, false);
        let note = cell(&table, "screen:0", "note");
        assert_eq!(note.as_deref(), Some(RECORDED_ONLY));
        let machine = cell(&table, "screen:0", "machine");
        assert_eq!(
            machine.as_deref(),
            Some("ci-ubuntu"),
            "the note keeps its row"
        );
    }

    #[test]
    fn a_machine_with_no_recorded_screen_says_so_and_a_refused_picture_says_why() {
        let (table, _) = screened(vec![adapter("recorded"), open(Some("db-01"))], false);
        let why = FeedStatus::Error("db-01 has no recorded screen".to_owned());
        assert_eq!(table.status(), &why);
        let (table, _) = screened(vec![adapter("recorded"), open(None)], true);
        let note = cell(&table, "screen:0", "note").unwrap_or_default();
        assert!(note.contains("too big"), "{note}");
        assert_eq!(table.status(), &FeedStatus::Live);
    }
}
