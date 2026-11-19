//! The contracts every later task builds on, each checked as a user of it would meet it.

use std::collections::BTreeSet;
use std::error::Error;

use yd_core::address::uri::{Address, AddressError};
use yd_core::command::table::{FLIP_CHAT, Route, Run};
use yd_core::feed::{FeedEvent, FeedStatus, MAX_ROWS};
use yd_core::grid::layout::{ChatSide, MIN_WINDOW, Rect, Size};
use yd_core::grid::place::place;
use yd_core::grid::saved::{EVERYTHING, SAVED};
use yd_core::ids::{FeedId, Millis, PaneId, RowId};
use yd_core::inbox::inbox;
use yd_core::kind::{Kind, RESERVED};
use yd_core::model::Model;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Dot, Row, Rows, Tone, Value, rows_from_json};
use yd_core::tokens::{ThemeFile, ThemeValue, TokenError, Tokens, rgb};

type Check = Result<(), Box<dyn Error>>;

const FEED: FeedId = FeedId::new("daemons");

/// A link copied from the app and pasted back must find the same thing.
#[test]
fn an_address_round_trips() -> Check {
    let plain = Address::new(Kind::Container, "atlas-api-db-1")?;
    assert_eq!(plain.to_string(), "yi://container/atlas-api-db-1");
    let lines = Address::new(Kind::Lines, "src/middleware/rate_limit.rs")?.part("L13-L20");
    assert_eq!(
        lines.to_string(),
        "yi://lines/src/middleware/rate_limit.rs#L13-L20"
    );
    let awkward = Address::new(Kind::File, "notes/50% done #2 – 日本.md")?.part("a b");
    let turned = Address::new(Kind::File, "a\u{202E}gpj.exe")?;
    assert_eq!(
        turned.to_string(),
        "yi://file/a%E2%80%AEgpj.exe",
        "a direction override is drawn"
    );
    for address in [plain, lines, awkward, turned] {
        let text = address.to_string();
        assert!(!text.contains(' '), "an address is one word: {text}");
        assert_eq!(text.parse::<Address>()?, address, "{text}");
    }
    Ok(())
}

/// Yi reads `yi://tools` and `yi://skills` itself; the app must never mint or accept them.
#[test]
fn an_address_never_takes_a_root_yi_owns() {
    for root in RESERVED {
        assert_eq!(Kind::from_slug(root), None, "{root} is a kind");
        let parsed = format!("yi://{root}/read").parse::<Address>();
        assert_eq!(parsed, Err(AddressError::Reserved), "{root}");
    }
    assert_eq!(
        "yi://gadget/1".parse::<Address>(),
        Err(AddressError::UnknownKind)
    );
    assert_eq!(
        "https://x/y".parse::<Address>(),
        Err(AddressError::NoScheme)
    );
    assert_eq!("yi://file/".parse::<Address>(), Err(AddressError::NoId));
    assert_eq!(
        "yi://file/a%zz".parse::<Address>(),
        Err(AddressError::BadEscape)
    );
    let long = format!("yi://file/{}", "a".repeat(4096));
    assert!(matches!(
        long.parse::<Address>(),
        Err(AddressError::TooLong { .. })
    ));
}

#[test]
fn every_kind_has_its_own_slug() {
    let slugs: BTreeSet<&str> = Kind::ALL.iter().map(|kind| kind.slug()).collect();
    assert_eq!(slugs.len(), Kind::ALL.len(), "two kinds share a slug");
    for kind in Kind::ALL {
        assert_eq!(Kind::from_slug(kind.slug()), Some(*kind));
    }
}

fn sample() -> Vec<Msg> {
    let row = |id: &str, name: &str| {
        Row::new(id)
            .text("name", name)
            .cell("state", Value::Dot(Dot::Run))
    };
    vec![
        Msg::Tick(Millis(16)),
        Msg::Feed(
            FEED,
            FeedEvent::Reset(vec![row("a", "alpha"), row("b", "beta")]),
        ),
        Msg::Feed(FEED, FeedEvent::Upsert(row("c", "gamma"))),
        Msg::Feed(FEED, FeedEvent::Upsert(row("a", "alpha, renamed"))),
        Msg::Feed(FEED, FeedEvent::Remove(RowId::new("b"))),
        Msg::Command(Route::Model, Run::new(FLIP_CHAT)),
        Msg::Feed(FEED, FeedEvent::Status(FeedStatus::Offline)),
    ]
}

/// Replay and tests rely on this: the same messages always build the same model.
#[test]
fn the_same_input_gives_the_same_step() {
    let (mut one, mut two) = (Model::default(), Model::default());
    for msg in sample() {
        let (a, b) = (step(&mut one, msg.clone()), step(&mut two, msg));
        assert_eq!(a, b, "effects differ");
        assert_eq!(one, two, "models differ");
    }
    assert_ne!(one, Model::default(), "the messages changed nothing");
}

/// A list keeps a row's place when it updates, and keeps the rows when its source goes away.
#[test]
fn a_feed_folds_into_its_table() -> Check {
    let mut model = Model::default();
    let mut versions = BTreeSet::new();
    for msg in sample() {
        step(&mut model, msg);
        versions.insert(model.tables.get(FEED).map_or(0, Rows::version));
    }
    let table = model.tables.get(FEED).ok_or("the feed has no table")?;
    let names: Vec<_> = (0..table.len())
        .filter_map(|i| table.get(i)?.words("name"))
        .collect();
    assert_eq!(names, ["alpha, renamed", "gamma"]);
    assert_eq!(
        table
            .find(&RowId::new("c"))
            .and_then(|row| row.words("name")),
        Some("gamma")
    );
    assert_eq!(table.find(&RowId::new("b")), None);
    assert_eq!(table.status(), &FeedStatus::Offline);
    assert_eq!(
        versions.len(),
        6,
        "every change to the table moves its version"
    );
    let fresh = vec![
        Row::new("x").text("name", "chi"),
        Row::new("y").text("name", "psi"),
    ];
    step(&mut model, Msg::Feed(FEED, FeedEvent::Reset(fresh)));
    let table = model.tables.get(FEED).ok_or("the feed has no table")?;
    assert_eq!(
        table
            .find(&RowId::new("y"))
            .and_then(|row| row.words("name")),
        Some("psi")
    );
    assert_eq!(
        table.find(&RowId::new("a")),
        None,
        "a row from before the reset is still found"
    );
    assert_eq!(table.status(), &FeedStatus::Live);
    Ok(())
}

/// A feed that never stops (logs) must not grow without bound, and must say what it let go.
#[test]
fn a_table_lets_its_oldest_rows_go_at_the_cap() -> Check {
    let mut model = Model::default();
    let full = (0..MAX_ROWS).map(|i| Row::new(format!("r{i}"))).collect();
    step(&mut model, Msg::Feed(FEED, FeedEvent::Reset(full)));
    let table = model.tables.get(FEED).ok_or("the feed has no table")?;
    assert_eq!(
        (table.len(), table.dropped()),
        (MAX_ROWS, 0),
        "at the cap nothing goes"
    );
    step(
        &mut model,
        Msg::Feed(FEED, FeedEvent::Upsert(Row::new("one-more"))),
    );
    let table = model.tables.get(FEED).ok_or("the feed has no table")?;
    let gone = MAX_ROWS / 8;
    assert_eq!(
        (table.len(), table.dropped()),
        (MAX_ROWS + 1 - gone, gone as u64)
    );
    assert_eq!(
        table.find(&RowId::new("r0")),
        None,
        "the oldest row is still held"
    );
    let first = table.get(0).map(|row| row.id.as_str().to_owned());
    assert_eq!(first, Some(format!("r{gone}")));
    let last = RowId::new("one-more");
    assert_eq!(table.find(&last).map(|row| &row.id), Some(&last));
    let kept = RowId::new(format!("r{}", MAX_ROWS - 1));
    assert_eq!(
        table.find(&kept).map(|row| &row.id),
        Some(&kept),
        "a kept row is lost by id"
    );
    Ok(())
}

/// A source on its own thread reaches the model only through the inbox, in order.
#[test]
fn rows_sent_from_a_thread_reach_the_model() -> Check {
    let (sender, inbox) = inbox(|| {});
    let out = sender.feed(FEED);
    let worker = std::thread::spawn(move || {
        out.send(FeedEvent::Reset(vec![Row::new("a").text("name", "alpha")]))?;
        out.send(FeedEvent::Status(FeedStatus::Live))
    });
    worker.join().map_err(|_| "the source thread panicked")??;
    let mut model = Model::default();
    let drained = inbox.drain(&mut model);
    assert_eq!(drained.steps, 2);
    let table = model.tables.get(FEED).ok_or("the feed has no table")?;
    assert_eq!((table.len(), table.status()), (1, &FeedStatus::Live));
    Ok(())
}

#[test]
fn a_fixture_reads_as_rows() -> Check {
    let rows = rows_from_json(
        r#"[{"id": "db", "cells": {"name": {"text": "postgres"}, "state": {"dot": "run"},
            "cpu": {"meter": {"ratio": 0.41, "text": "41%"}}, "tags": {"tags": ["a", "b"]},
            "level": {"badge": ["High", "bad"]}, "stage": {"pill": ["New", "context"]},
            "took": {"bar": {"start": 0.1, "len": 0.4, "tone": "model"}},
            "login": {"flag": true}, "load": {"series": [1, 2.5]}}},
           {"id": "old", "parent": "db", "dim": true}]"#,
    )?;
    let [db, old] = rows.as_slice() else {
        return Err("two rows expected".into());
    };
    assert_eq!(db.words("name"), Some("postgres"));
    assert_eq!(db.get("state"), Some(&Value::Dot(Dot::Run)));
    assert_eq!(db.words("cpu"), Some("41%"));
    assert_eq!(
        db.get("level"),
        Some(&Value::Badge("High".to_owned(), Tone::Bad))
    );
    assert_eq!(
        db.get("stage"),
        Some(&Value::Pill("New".to_owned(), Tone::Context))
    );
    let bar = Value::Bar {
        start: 0.1,
        len: 0.4,
        tone: Tone::Model,
    };
    assert_eq!(
        (db.get("took"), db.get("login")),
        (Some(&bar), Some(&Value::Flag(true)))
    );
    assert_eq!(db.get("load"), Some(&Value::Series(vec![1.0, 2.5])));
    assert_eq!(
        (old.parent.as_ref(), old.dim),
        (Some(&RowId::new("db")), true)
    );
    assert!(
        rows_from_json(r#"[{"cells": {}}]"#).is_err(),
        "a row with no id was taken"
    );
    Ok(())
}

fn overlap(a: Rect, b: Rect) -> f32 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    w.max(0.0) * h.max(0.0)
}

/// No hole and no overlap, at the reference window and at the smallest one, chat on either side.
#[test]
fn the_saved_layouts_tile_the_window_exactly() {
    let sizes = [
        Size {
            w: 1200.0,
            h: 750.0,
        },
        Size {
            w: 1042.0,
            h: 708.0,
        },
        MIN_WINDOW,
        Size { w: 562.0, h: 438.0 },
    ];
    for (saved, size, side) in SAVED.iter().flat_map(|saved| {
        sizes
            .into_iter()
            .flat_map(move |size| [ChatSide::Right, ChatSide::Left].map(|side| (saved, size, side)))
    }) {
        let layout = saved.open(|kind| PaneId(kind as u32));
        let placed = place(&layout, size, side);
        let at = format!("{} at {} by {}, chat {side:?}", saved.name, size.w, size.h);
        assert_eq!(
            placed.len(),
            saved.areas.len(),
            "{at}: a pane has no rectangle"
        );
        let mut covered = 0.0;
        for (i, (pane, rect)) in placed.iter().enumerate() {
            assert!(
                rect.w > 0.0 && rect.h > 0.0,
                "{at}: {pane:?} is empty: {rect:?}"
            );
            let inside = rect.x >= -0.01
                && rect.y >= -0.01
                && rect.x + rect.w <= size.w + 0.01
                && rect.y + rect.h <= size.h + 0.01;
            assert!(inside, "{at}: {pane:?} leaves the grid: {rect:?}");
            covered += rect.w * rect.h;
            for (other, next) in placed.iter().skip(i + 1) {
                assert!(
                    overlap(*rect, *next) < 0.01,
                    "{at}: {pane:?} overlaps {other:?}"
                );
            }
        }
        assert!(
            (covered - size.w * size.h).abs() < 0.5,
            "{at}: covered {covered}"
        );
    }
}

/// The record's own pixel result for Everything on the frame 1 grid (2.7.4), then mirrored (2.7.6).
#[test]
fn everything_places_panes_as_the_record_measures_them() -> Check {
    let layout = EVERYTHING.open(|kind| PaneId(kind as u32));
    let grid = Size {
        w: 1034.0,
        h: 676.0,
    };
    let find = |side: ChatSide, kind: PaneKind| {
        let placed = place(&layout, grid, side);
        let rect = placed
            .iter()
            .find(|(pane, _)| *pane == PaneId(kind as u32))
            .map(|(_, rect)| *rect);
        rect.ok_or(format!("{kind:?} has no rectangle"))
    };
    let rect = |x, y, w, h| Rect { x, y, w, h };
    assert_eq!(
        find(ChatSide::Right, PaneKind::Files)?,
        rect(0.0, 0.0, 208.0, 676.0)
    );
    assert_eq!(
        find(ChatSide::Right, PaneKind::Changes)?,
        rect(208.0, 0.0, 270.0, 416.0)
    );
    assert_eq!(
        find(ChatSide::Right, PaneKind::Chat)?,
        rect(764.0, 226.0, 270.0, 450.0)
    );
    assert_eq!(
        find(ChatSide::Left, PaneKind::Chat)?,
        rect(0.0, 226.0, 270.0, 450.0)
    );
    assert_eq!(
        find(ChatSide::Left, PaneKind::Files)?,
        rect(826.0, 0.0, 208.0, 676.0)
    );
    Ok(())
}

/// The one command built so far: the menu item that moves chat to the other side.
#[test]
fn a_command_changes_the_model() {
    let mut model = Model::default();
    assert_eq!(model.grid.side, ChatSide::Left, "the owner's default");
    let effects = step(&mut model, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
    assert_eq!((model.grid.side, effects.len()), (ChatSide::Right, 0));
    step(&mut model, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
    assert_eq!(model.grid.side, ChatSide::Left);
}

/// Graphite from the record (7.4): the values it names change, the rest stay Yi Dark.
#[test]
fn a_theme_file_sets_tokens_by_name() -> Check {
    let file = ThemeFile::parse(
        r##"{"id": "graphite", "name": "Graphite", "note": "grey, nearly solid panes", "dark": true,
            "values": {"text.1": "#d5d6da", "accent": "#a9b0bd", "glass.pane": 0.62,
                       "font.text": "system", "glass": false, "bogus": 1, "ok": 7}}"##,
    )?;
    let (tokens, refused) = Tokens::themed(&file);
    assert_eq!(
        (tokens.fg, tokens.accent),
        (rgb(0xd5, 0xd6, 0xda), rgb(0xa9, 0xb0, 0xbd))
    );
    assert_eq!((tokens.glass_pane, tokens.glass), (0.62, false));
    assert_eq!(
        tokens.bad,
        Tokens::default().bad,
        "a value the theme left alone changed"
    );
    assert_eq!(refused, [("ok".to_owned(), TokenError::NotAColour)]);
    assert_eq!(
        tokens.number_or("bogus", 0.0),
        1.0,
        "a pane's own token is not kept"
    );
    assert_eq!(tokens.number_or("absent", 12.0), 12.0);
    let huge = ThemeValue::Number(1e300);
    assert_eq!(
        Tokens::default().set("lines", &huge),
        Err(TokenError::NotANumber)
    );
    let names: BTreeSet<&str> = Tokens::NAMES.iter().copied().collect();
    assert_eq!(names.len(), Tokens::NAMES.len(), "two tokens share a name");
    for (name, value) in Tokens::default().values() {
        let mut again = Tokens::default();
        again.set(name, &value)?;
        assert_eq!(
            again,
            Tokens::default(),
            "{name} does not read back what it wrote"
        );
    }
    Ok(())
}
