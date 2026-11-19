//! A thing: what a keyed element on screen stands for, and the one name each kind goes by.

use crate::address::uri::Address;
use crate::feed::FeedStatus;
use crate::findings::triage::FindingId;
use crate::findings::view;
use crate::ids::{FeedId, PaneId, RowId, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::pane::PaneKind;
use crate::row::Row;

#[derive(Debug, Clone, PartialEq)]
pub struct Thing {
    /// What a chip and a menu header call it. For people; never parsed.
    pub label: String,
    pub address: Address,
    /// A few words of its content, sent with it when it is attached to a prompt.
    pub excerpt: Option<String>,
}

impl Thing {
    pub fn kind(&self) -> Kind {
        self.address.kind()
    }

    /// The thing at `address`, known only by where it lives: named by its kind's rule, with
    /// the words of its row when a feed holds one.
    pub fn at(model: &Model, address: Address) -> Thing {
        let finding = || {
            let id = FindingId::new(address.id());
            let found = (address.kind() == Kind::Finding).then(|| view::find(model, &id));
            found.flatten().map(|finding| finding.words())
        };
        let said = row(model, &address).map(words).filter(|w| !w.is_empty());
        let said = said.or_else(finding);
        Thing {
            label: label(model, &address, None),
            address,
            excerpt: said,
        }
    }

    /// This thing under the one name its kind goes by, whatever the engine that found it
    /// called it, so a chip, a box's tag and a menu's header all say the same.
    pub fn named(self, model: &Model) -> Thing {
        Thing {
            label: label(model, &self.address, Some(&self.label)),
            ..self
        }
    }
}

/// The feed whose rows are the records of `kind`: a list pane's feed is named by its slug.
fn feed(kind: Kind) -> Option<FeedId> {
    let pane = match kind {
        Kind::Agent => PaneKind::Agents,
        Kind::Span => PaneKind::Trace,
        Kind::LogLine => PaneKind::Logs,
        Kind::Finding => PaneKind::Findings,
        Kind::Asset => PaneKind::Assets,
        Kind::Container => PaneKind::Containers,
        Kind::Vm => PaneKind::Vms,
        Kind::Service => PaneKind::Daemons,
        Kind::Plugin => PaneKind::Plugins,
        Kind::LanguageServer => PaneKind::LanguageServers,
        Kind::Theme => PaneKind::Themes,
        Kind::Lines
        | Kind::File
        | Kind::Comment
        | Kind::Step
        | Kind::Command
        | Kind::Approval
        | Kind::Answer
        | Kind::Message
        | Kind::Thread
        | Kind::Turn
        | Kind::Card
        | Kind::Element
        | Kind::Screen
        | Kind::Page
        | Kind::Reference
        | Kind::StackFrame
        | Kind::Variable
        | Kind::Breakpoint
        | Kind::Notification
        | Kind::Setting
        | Kind::ThemeValue
        | Kind::Number
        | Kind::Check
        | Kind::Place
        | Kind::StandingCheck
        | Kind::Problem
        | Kind::Property
        | Kind::Tab
        | Kind::Pane => return None,
    };
    Some(FeedId::new(pane.slug()))
}

fn row<'a>(model: &'a Model, address: &Address) -> Option<&'a Row> {
    let table = model.tables.get(feed(address.kind())?)?;
    table.find(&RowId::new(address.id()))
}

/// The words of a row's cells, in the order of their field names.
fn words(row: &Row) -> String {
    let said: Vec<&str> = row.cells.keys().filter_map(|f| row.words(f)).collect();
    said.join(" · ")
}

/// The lines a part names: `L13` is 13 to 13, `L13-L20` is 13 to 20.
pub fn lines(part: &str) -> Option<(u32, u32)> {
    let number = |text: &str| text.strip_prefix('L')?.parse::<u32>().ok();
    match part.split_once('-') {
        Some((first, last)) => Some((number(first)?, number(last)?)),
        None => number(part).map(|line| (line, line)),
    }
}

/// What a thing is called (record 4.8): lines by their file and range, a thread by its title,
/// a web element by its selector, and every other kind by its kind and its own name.
fn label(model: &Model, address: &Address, given: Option<&str>) -> String {
    let (kind, id, part) = (address.kind(), address.id(), address.part_text());
    let pane = id.parse().ok().and_then(|n| model.grid.kind(PaneId(n)));
    let pane = pane.filter(|_| kind == Kind::Pane).map(PaneKind::title);
    let name = pane.unwrap_or_else(|| id.rsplit('/').next().unwrap_or(id));
    if kind == Kind::Lines {
        return match part.and_then(lines) {
            Some((first, last)) if first == last => format!("{name} line {first}"),
            Some((first, last)) => format!("{name} lines {first}–{last}"),
            None => name.to_owned(),
        };
    }
    if kind == Kind::Thread {
        let thread = model.wire.threads.get(&ThreadId::new(id));
        let title = thread.and_then(|thread| thread.title.as_deref());
        return format!("thread · {}", title.unwrap_or(id));
    }
    if kind == Kind::Element {
        return part.or(given).unwrap_or(id).to_owned();
    }
    if kind == Kind::Screen {
        return match (given, part) {
            (Some(given), _) => given.to_owned(),
            (None, Some(part)) => format!("{id} · {part}"),
            (None, None) => id.to_owned(),
        };
    }
    let head = [&kind.slug().replace('-', " "), " · "].concat();
    let said = match given {
        Some(given) if kind != Kind::File => given,
        Some(_) | None => name,
    };
    match said.strip_prefix(head.as_str()) {
        Some(_) => said.to_owned(),
        None => [head.as_str(), said].concat(),
    }
}

/// Whether the record a thing stands for is no longer there: a thread the server dropped, a
/// row its settled feed no longer has. A kind the model keeps no full record of is never gone.
pub fn gone(model: &Model, address: &Address) -> bool {
    if address.kind() == Kind::Thread {
        let id = ThreadId::new(address.id());
        return !model.wire.threads.contains_key(&id);
    }
    let Some(table) = feed(address.kind()).and_then(|feed| model.tables.get(feed)) else {
        return false;
    };
    let settled = matches!(table.status(), FeedStatus::Live | FeedStatus::Empty);
    settled && table.find(&RowId::new(address.id())).is_none()
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::feed::FeedEvent;
    use crate::msg::{Msg, step};

    type Check = Result<(), Box<dyn Error>>;

    /// Every kind the record lists (4.8), the record its id is, and a sample address.
    const FORMS: &[(Kind, &str, &str)] = &[
        (
            Kind::Lines,
            "repository path, part L<a> or L<a>-L<b>",
            "yi://lines/src/middleware/rate_limit.rs#L13-L20",
        ),
        (
            Kind::File,
            "repository path",
            "yi://file/src/routes/upload.rs",
        ),
        (
            Kind::Comment,
            "comment id",
            "yi://comment/01JCN0V7T2-comment-3",
        ),
        (
            Kind::Step,
            "thread id / tool call id",
            "yi://step/019a8e1c-thread/toolu_01Hq",
        ),
        (
            Kind::Command,
            "thread id / tool call id",
            "yi://command/019a8e1c-thread/toolu_01Hq",
        ),
        (
            Kind::Approval,
            "the server's ask id",
            "yi://approval/perm_019a8e1c_4",
        ),
        (
            Kind::Answer,
            "thread id / entry id, part a paragraph",
            "yi://answer/019a8e1c-thread/01a1de97-entry#p2",
        ),
        (
            Kind::Message,
            "thread id / entry id",
            "yi://message/019a8e1c-thread/01a1de97-entry",
        ),
        (Kind::Thread, "thread id", "yi://thread/019a8e1c-thread"),
        (
            Kind::Turn,
            "thread id / the entry that began it",
            "yi://turn/019a8e1c-thread/01a1de97-entry",
        ),
        (
            Kind::Card,
            "thread id / entry id",
            "yi://card/019a8e1c-thread/01a1de97-entry",
        ),
        (
            Kind::Element,
            "page host and path, part a selector",
            "yi://element/localhost:5173/upload#div.alert",
        ),
        (
            Kind::Screen,
            "machine id, part what on its screen",
            "yi://screen/ci-ubuntu#window/terminal",
        ),
        (
            Kind::Page,
            "thread id / page id, part a block of it",
            "yi://page/019a8e1c-thread/report-1#h2-results",
        ),
        (Kind::Agent, "agent id", "yi://agent/tests-writer-2"),
        (Kind::Span, "span id", "yi://span/turn-2.tool-7"),
        (
            Kind::LogLine,
            "log row id",
            "yi://log-line/17:57:58.101-0042",
        ),
        (Kind::Finding, "finding id", "yi://finding/F12"),
        (Kind::Asset, "asset row id", "yi://asset/atlas-api-image"),
        (Kind::Container, "container id", "yi://container/atlas-api"),
        (Kind::Vm, "machine id", "yi://vm/ci-ubuntu"),
        (Kind::Service, "launchd label", "yi://service/com.yi.daemon"),
        (
            Kind::Reference,
            "repository path, part a line",
            "yi://reference/src/routes/upload.rs#L23",
        ),
        (
            Kind::StackFrame,
            "debug session / frame id",
            "yi://stack-frame/session-1/1004",
        ),
        (
            Kind::Variable,
            "debug session / variable reference",
            "yi://variable/session-1/1004/tokens",
        ),
        (
            Kind::Breakpoint,
            "repository path, part a line",
            "yi://breakpoint/src/middleware/rate_limit.rs#L16",
        ),
        (
            Kind::Notification,
            "notification id",
            "yi://notification/n-0031",
        ),
        (Kind::Plugin, "plugin id", "yi://plugin/yi.containers-pane"),
        (
            Kind::Setting,
            "setting key",
            "yi://setting/appearance.glass",
        ),
        (Kind::Theme, "theme file id", "yi://theme/graphite"),
        (
            Kind::ThemeValue,
            "theme file id, part a token name",
            "yi://theme-value/graphite#text.1",
        ),
        (
            Kind::Number,
            "pane id / the script's own id",
            "yi://number/12/p95-latency",
        ),
        (Kind::Check, "check id", "yi://check/cargo-test-api"),
        (Kind::Place, "folder path", "yi://place//work/atlas-api"),
        (
            Kind::StandingCheck,
            "standing check id",
            "yi://standing-check/no-unwrap",
        ),
        (
            Kind::Problem,
            "repository path, part line and code",
            "yi://problem/src/routes/upload.rs#L23:E0308",
        ),
        (
            Kind::LanguageServer,
            "server id",
            "yi://language-server/rust-analyzer",
        ),
        (
            Kind::Property,
            "page host and path, part selector and property",
            "yi://property/localhost:5173/upload#div.alert/border-radius",
        ),
        (Kind::Tab, "pane id / tab id", "yi://tab/4/2"),
        (Kind::Pane, "pane id", "yi://pane/4"),
    ];

    #[test]
    fn every_kind_has_an_address_form_that_round_trips_and_a_name() -> Check {
        let listed: Vec<Kind> = FORMS.iter().map(|(kind, ..)| *kind).collect();
        assert_eq!(listed, Kind::ALL, "one form a kind, in the kinds' order");
        let model = Model::default();
        for (kind, form, example) in FORMS {
            let address: Address = example.parse()?;
            assert_eq!(address.kind(), *kind, "{example}");
            assert_eq!(&address.to_string(), example, "written back as it was read");
            let parts = (form.contains("part"), address.part_text().is_some());
            assert!(parts.0 == parts.1, "{example} against its form: {form}");
            let thing = Thing::at(&model, address.clone());
            assert!(!thing.label.is_empty(), "{example} has a name");
            let again = thing.clone().named(&model);
            assert_eq!(again.label, thing.label, "{example} is named once");
            assert_eq!(thing.address, address);
        }
        Ok(())
    }

    #[test]
    fn a_thing_goes_by_the_name_of_its_kind() -> Check {
        let mut model = Model::default();
        let at = |model: &Model, address: &str| -> Result<String, Box<dyn Error>> {
            Ok(Thing::at(model, address.parse()?).label)
        };
        let range = "yi://lines/src/middleware/rate_limit.rs#L13-L20";
        assert_eq!(at(&model, range)?, "rate_limit.rs lines 13–20");
        let one = "yi://lines/src/middleware/rate_limit.rs#L13";
        assert_eq!(at(&model, one)?, "rate_limit.rs line 13");
        let element = "yi://element/localhost:5173/upload#div.alert";
        assert_eq!(at(&model, element)?, "div.alert");
        let file = "yi://file/src/routes/upload.rs";
        assert_eq!(at(&model, file)?, "file · upload.rs");
        assert_eq!(at(&model, "yi://log-line/a1")?, "log line · a1");
        // An engine's own name for a row is kept, under the row's kind.
        let found = Thing {
            label: "atlas-api".to_owned(),
            address: "yi://container/c0ffee".parse()?,
            excerpt: None,
        };
        assert_eq!(found.named(&model).label, "container · atlas-api");
        let window = Thing {
            label: "ci-ubuntu · Terminal window".to_owned(),
            address: "yi://screen/ci-ubuntu#window/terminal".parse()?,
            excerpt: None,
        };
        assert_eq!(window.named(&model).label, "ci-ubuntu · Terminal window");
        // A thread is called by the title the server gives it.
        let untitled = "yi://thread/fork-sliding-window";
        assert_eq!(at(&model, untitled)?, "thread · fork-sliding-window");
        crate::wire::replay::play(&mut model, LISTED)?;
        assert_eq!(at(&model, untitled)?, "thread · Fork · sliding window");
        Ok(())
    }

    const LISTED: &str = r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":2}}}
{"dir":"recv","frame":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"},"capabilities":{},"authMethods":[]}}}
{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/list","params":{}}}
{"dir":"recv","frame":{"jsonrpc":"2.0","id":2,"result":{"sessions":[{"sessionId":"fork-sliding-window","cwd":"/work/atlas-api","title":"Fork · sliding window"}]}}}"#;

    #[test]
    fn a_thing_whose_record_went_is_gone_and_one_never_recorded_is_not() -> Check {
        let mut model = Model::default();
        let thread: Address = "yi://thread/fork-sliding-window".parse()?;
        assert!(gone(&model, &thread), "no such thread");
        crate::wire::replay::play(&mut model, LISTED)?;
        assert!(!gone(&model, &thread));
        let container: Address = "yi://container/atlas-api".parse()?;
        assert!(!gone(&model, &container), "no feed has said anything yet");
        let feed = FeedId::new("containers");
        let row = Row::new("atlas-api")
            .text("image", "atlas/api")
            .text("name", "atlas-api");
        step(&mut model, Msg::Feed(feed, FeedEvent::Reset(vec![row])));
        step(
            &mut model,
            Msg::Feed(feed, FeedEvent::Status(FeedStatus::Live)),
        );
        assert!(!gone(&model, &container));
        let kept = Thing::at(&model, container.clone());
        assert_eq!(kept.excerpt.as_deref(), Some("atlas/api · atlas-api"));
        step(
            &mut model,
            Msg::Feed(feed, FeedEvent::Remove(RowId::new("atlas-api"))),
        );
        assert!(gone(&model, &container), "its row left a live feed");
        let offline = FeedEvent::Status(FeedStatus::Offline);
        step(&mut model, Msg::Feed(feed, offline));
        assert!(!gone(&model, &container), "an offline feed proves nothing");
        let lines: Address = "yi://lines/src/lib.rs#L1".parse()?;
        assert!(!gone(&model, &lines), "the model lists no file for certain");
        Ok(())
    }
}
