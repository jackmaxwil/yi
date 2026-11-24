//! Design mode through the browser source, against a scripted page: what a pick, a change, a
//! reset and a reload put in the page's feed.

use std::error::Error;
use std::sync::mpsc;

use yd_core::command::table::Run;
use yd_core::command::tools::design::{AT, ELEMENT, RESET, SET};
use yd_core::command::tools::web;
use yd_core::feed::{SourceCmd, Table};
use yd_core::ids::{CommandId, RowId};
use yd_core::inbox::inbox;
use yd_core::model::Model;
use yd_core::row::Rows;

use super::{FEED, Missing, serve};
use crate::cdp::{Element, Scripted};

type Check = Result<(), Box<dyn Error>>;

const WEB: &str = include_str!("../../../../fixtures/mockup/panes/web.text.json");
const DESIGN: &str = include_str!("../../../../fixtures/mockup/panes/design.text.json");

fn strings(fixture: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let read: serde_json::Value = serde_json::from_str(fixture)?;
    let listed = read.get("strings").and_then(|s| s.as_array());
    let words = listed.ok_or("no strings")?.iter();
    Ok(words
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect())
}

/// The mockup's alert as the page first draws it: the words of frame 1, the old values the
/// Design pane strikes through, and the record's place for it (4.9.1).
fn alert() -> Result<Scripted, Box<dyn Error>> {
    let (web, design) = (strings(WEB)?, strings(DESIGN)?);
    let at = |i: usize| design.get(i).cloned().ok_or("the fixture is short");
    let (padding, radius) = ((at(10)?.parse()?, at(11)?.parse()?), at(20)?.parse()?);
    assert_eq!((at(15)?.as_str(), at(18)?.as_str()), ("#fdecec", "#9c2424"));
    Ok(Scripted::of(Element {
        name: at(3)?,
        source: at(4)?,
        at: (16.0, 60.0),
        size: (602.0, 30.0),
        padding,
        fill: "rgb(253, 236, 236)".to_owned(),
        ink: "rgb(156, 36, 36)".to_owned(),
        radius,
        says: web.get(9).cloned().ok_or("no alert")?,
        gap: 10.0,
        inset: 16.0,
    }))
}

fn ask(command: CommandId, text: &str) -> SourceCmd {
    SourceCmd::Run(Run {
        text: Some(text.to_owned()),
        ..Run::new(command)
    })
}

/// The page's table after the source has carried out `asks` on the mockup's page.
fn played(asks: Vec<SourceCmd>) -> Result<Table, Box<dyn Error>> {
    let (sender, inbox) = inbox(|| {});
    let (cmds, rx) = mpsc::channel();
    cmds.send(ask(web::OPEN, "localhost:5173/upload"))?;
    for ask in asks {
        cmds.send(ask)?;
    }
    drop(cmds);
    let page = alert()?;
    let launch = |url: &str, _: (u32, u32), _: u32| {
        let mut page = page.clone();
        super::Page::navigate(&mut page, url);
        Ok::<_, Missing>(page)
    };
    serve(launch, |_| Ok(()), sender.feed(FEED), rx)?;
    let mut model = Model::default();
    inbox.drain(&mut model);
    Ok(model.tables.get(FEED).cloned().unwrap_or_default())
}

fn cell<'a>(table: &'a Table, row: &str, cell: &str) -> Option<&'a str> {
    table.find(&RowId::new(row))?.words(cell)
}

/// The four edits of frame 24, as the Design pane sends them.
fn edits() -> Result<Vec<SourceCmd>, Box<dyn Error>> {
    let design = strings(DESIGN)?;
    let at = |i: usize| design.get(i).map(String::as_str).unwrap_or_default();
    Ok(vec![
        ask(AT, "20,70"),
        ask(SET, &format!("fill={}", at(14))),
        ask(SET, &format!("text={}", at(17))),
        ask(SET, "corners=10"),
        ask(SET, &format!("says={}", at(23))),
    ])
}

#[test]
fn a_pick_describes_the_element_under_the_point_and_a_miss_says_nothing_is_there() -> Check {
    let table = played(vec![ask(AT, "20,70")])?;
    let at = |name: &str| cell(&table, ELEMENT, name);
    assert_eq!(at("name"), Some("div.alert"));
    assert_eq!(at("source"), Some("src/pages/Upload.tsx:41"));
    assert_eq!(at("page"), Some("localhost:5173/upload"));
    assert_eq!(
        (at("size"), at("padding")),
        (Some("602 × 30"), Some("7 10"))
    );
    assert_eq!((at("fill"), at("text")), (Some("#fdecec"), Some("#9c2424")));
    assert_eq!(
        (at("corners"), at("gap"), at("inset")),
        (Some("6"), Some("10"), Some("16"))
    );
    assert_eq!(
        (at("x"), at("y"), at("w"), at("h")),
        (Some("16"), Some("60"), Some("602"), Some("30"))
    );
    assert_eq!(
        table.len(),
        2,
        "the page's row and the element's, no change"
    );

    let missed = played(vec![ask(AT, "5,5")])?;
    let note = cell(&missed, ELEMENT, "note");
    assert_eq!(note, Some("Nothing on the page is there to pick."));
    Ok(())
}

#[test]
fn a_new_size_is_what_the_page_reports_and_is_listed_as_a_change() -> Check {
    let table = played(vec![ask(AT, "20,70"), ask(SET, "size=640 × 36")])?;
    assert_eq!(cell(&table, ELEMENT, "size"), Some("640 × 36"));
    assert_eq!(cell(&table, ELEMENT, "size_was"), Some("602 × 30"));
    assert_eq!(cell(&table, ELEMENT, "w"), Some("640"));
    let change = |name: &str| cell(&table, "change:0:size", name);
    assert_eq!(
        (change("was"), change("now")),
        (Some("602 × 30"), Some("640 × 36"))
    );
    assert_eq!(change("element"), Some("div.alert"));
    Ok(())
}

#[test]
fn four_edits_are_four_change_rows_and_a_value_set_back_is_no_change() -> Check {
    let table = played(edits()?)?;
    let change = |slug: &str| {
        let id = format!("change:0:{slug}");
        let (was, now) = (cell(&table, &id, "was"), cell(&table, &id, "now"));
        was.zip(now)
    };
    assert_eq!(change("fill"), Some(("#fdecec", "#fff4e5")));
    assert_eq!(change("text"), Some(("#9c2424", "#8a4b00")));
    assert_eq!(change("corners"), Some(("6", "10")));
    let words = (
        "Too many uploads. Try again in 41 seconds.",
        "Slow down. Try again in 41 seconds.",
    );
    assert_eq!(change("says"), Some(words));
    assert_eq!(table.len(), 6, "the page, the element and four changes");

    let mut back = edits()?;
    back.push(ask(SET, "corners=6"));
    let table = played(back)?;
    assert_eq!(cell(&table, "change:0:corners", "now"), None);
    assert_eq!(table.len(), 5);
    Ok(())
}

#[test]
fn reset_puts_the_element_back_and_empties_the_change_list() -> Check {
    let mut asks = edits()?;
    asks.push(SourceCmd::Run(Run::new(RESET)));
    let table = played(asks)?;
    assert_eq!(table.len(), 2, "no change row is left");
    assert_eq!(cell(&table, ELEMENT, "fill"), Some("#fdecec"));
    assert_eq!(cell(&table, ELEMENT, "fill_was"), Some("#fdecec"));
    assert_eq!(cell(&table, ELEMENT, "corners"), Some("6"));
    Ok(())
}

#[test]
fn loading_the_page_again_drops_the_element_and_its_changes() -> Check {
    let mut asks = edits()?;
    asks.push(SourceCmd::Run(Run::new(web::RELOAD)));
    let table = played(asks)?;
    assert_eq!(table.len(), 1, "only the page's own row is left");
    assert_eq!(cell(&table, ELEMENT, "name"), None);
    Ok(())
}

#[test]
fn closing_the_page_takes_the_element_and_its_changes_with_it() -> Check {
    let mut asks = edits()?;
    asks.push(SourceCmd::Close);
    let table = played(asks)?;
    assert_eq!(cell(&table, ELEMENT, "name"), None);
    assert_eq!(cell(&table, "change:0:fill", "now"), None);
    Ok(())
}

#[test]
fn a_value_that_is_not_one_is_refused_with_a_note_and_changes_nothing() -> Check {
    let table = played(vec![ask(AT, "20,70"), ask(SET, "fill=red")])?;
    assert_eq!(cell(&table, ELEMENT, "fill"), Some("#fdecec"));
    let note = cell(&table, ELEMENT, "note");
    assert_eq!(note, Some("Not changed: “fill=red” is not a value"));
    assert_eq!(table.len(), 2);
    Ok(())
}
