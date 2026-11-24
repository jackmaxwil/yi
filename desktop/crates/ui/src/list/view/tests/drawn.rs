//! How a list is drawn: the record's sizes, the tree, groups, every kind of cell, the theme.

use std::error::Error;

use rune::{Color, Env, Runtime};
use yd_core::ids::{RowId, WindowId};
use yd_core::row::{Dot, Row, Tone, Value};
use yd_core::tokens::{ThemeValue, Tokens};

use super::{
    AGENTS, CONTAINERS, Check, DAEMONS, DEFS, LOGS, SIZE, Window, agents, boxed, containers, drawn,
    five, keyed, lines, logs, press, press_at, rect, root, says, service, service_key, services,
    settle, showing, sized, touched, window, window_of, worded,
};
use crate::list::view::SCENES;
use crate::theme::look::colour;
use crate::win::{Hub, Win};

/// Record 5.1.3 and 5.1.4: the bar is 22, a column header, a row and a footer are 20, a log
/// line is 16, and content starts 8 from the edge.
#[test]
fn a_list_takes_the_sizes_of_the_record() -> Check {
    let rt = window(services, vec![(DAEMONS, five())]);
    let all = rt.elements();
    let whole = keyed(&all, "daemons-7")?.rect;
    assert_eq!(whole, [0.0, 0.0, SIZE.0, SIZE.1]);
    let bar = keyed(&all, "daemons-7.pane-header")?.rect;
    assert_eq!((bar[1], bar[3]), (0.0, 22.0), "the pane's header bar");
    let head = keyed(&all, "daemons-7.table-header")?.rect;
    assert_eq!((head[1], head[3]), (22.0, 20.0), "the header row");
    let first = keyed(&all, &service_key("a"))?.rect;
    assert_eq!((first[1], first[3]), (42.0, 20.0), "the first row");
    let foot = keyed(&all, "daemons-7.footer")?.rect;
    assert_eq!((foot[1] + foot[3], foot[3]), (SIZE.1, 20.0), "the footer");
    assert!(says(&rt, "Services on this Mac"), "the footer's sentence");
    let name = worded(&all, "a")?.rect;
    assert_eq!(
        name[0],
        8.0 + 10.0 + 8.0,
        "the gutter, the dot column, one gap"
    );
    let pid = worded(&all, "4412")?.rect;
    assert!(
        (pid[0] + pid[2] - (SIZE.0 - 8.0)).abs() < 0.02,
        "a number ends at the right gutter: {pid:?}"
    );
    let log = window(logs, vec![(LOGS, lines(0..3))]);
    let line = rect(&log, "logs-7.row.r1")?;
    assert_eq!(line[3], 16.0, "a log line");
    assert!(
        !drawn(&log, "logs-7.table-header"),
        "a list with no column titles drew a header row"
    );
    Ok(())
}

/// Record 5.2.1: the row and its detail are one lit block with a 2 px bar down its left edge;
/// a selected row that is closed keeps the bar alone.
#[test]
fn the_open_row_and_its_detail_are_lit_as_one_block() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    let b = service_key("b");
    press(&mut rt, &b)?;
    let (all, lit) = (rt.elements(), colour(Tokens::default().selected));
    let (whole, detail) = (
        keyed(&all, &b)?.rect,
        keyed(&all, "daemons-7.detail.b")?.rect,
    );
    let block = all.iter().find(|e| e.rect == whole && e.fill == lit);
    assert!(
        block.is_some(),
        "no selection fill over the row and its detail"
    );
    let bar = keyed(&all, "daemons-7.cursor")?;
    assert_eq!(bar.rect, [whole[0], whole[1], 2.0, 20.0 + detail[3]]);
    assert_eq!(bar.fill, Color::WHITE.alpha(0.4));
    press(&mut rt, &b)?;
    let all = rt.elements();
    assert_eq!(
        keyed(&all, "daemons-7.cursor")?.rect,
        [whole[0], whole[1], 2.0, 20.0]
    );
    assert!(
        !all.iter().any(|e| e.fill == lit),
        "a closed row kept the selection fill"
    );
    Ok(())
}

/// Record 5.3: a child sits directly under its parent, one 12 px step in for each level, with
/// an elbow mark; the columns after the name stay in line.
#[test]
fn a_tree_draws_children_under_their_parent() -> Check {
    let agent = |id: &str, parent: Option<&str>| {
        let mut row = service(id, Dot::Work).text("doing", format!("{id} is busy"));
        row.parent = parent.map(RowId::new);
        row
    };
    let arrival = vec![
        agent("root", None),
        agent("fork", Some("root")),
        agent("tests", Some("root")),
        agent("docs", Some("root")),
        agent("bisect", Some("tests")),
    ];
    let rt = window(agents, vec![(AGENTS, arrival)]);
    let order: Vec<String> = showing(&rt, "agents-7")
        .into_iter()
        .map(|row| row.0)
        .collect();
    let want = ["root", "fork", "tests", "bisect", "docs"].map(|id| format!("agents-7.row.{id}"));
    assert_eq!(
        order, want,
        "a child that arrived late is not under its parent"
    );
    let all = rt.elements();
    let name = |id: &str| worded(&all, id).map(|e| e.rect[0]);
    let (root, tests, bisect) = (name("root")?, name("tests")?, name("bisect")?);
    assert_eq!(root, 8.0 + 7.0 + 8.0, "a root is not indented");
    assert!(
        tests >= root + 12.0 + 7.0,
        "a child at {tests}, its parent at {root}"
    );
    assert_eq!(bisect - tests, 12.0, "one nesting step");
    let doing = |id: &str| worded(&all, &format!("{id} is busy")).map(|e| e.rect[0]);
    assert_eq!(
        (doing("tests")?, doing("bisect")?),
        (doing("root")?, doing("root")?)
    );
    let elbows = sized(&all, 7.0, 9.0);
    assert_eq!(elbows.len(), 4, "one elbow for each row that has a parent");
    let bisect_row = keyed(&all, "agents-7.row.bisect")?.rect;
    let has_elbow = elbows.iter().any(|e| {
        e.rect[1] >= bisect_row[1] - 8.0 && e.rect[1] < bisect_row[1] + 20.0 && e.rect[0] < bisect
    });
    assert!(has_elbow, "bisect has no elbow before its name");
    Ok(())
}

/// Record 5.1.4: a group heading is 20 tall with 8 above it, its name then its summary; a
/// press on it opens nothing (5.2.2, rule 4).
#[test]
fn a_group_heading_stands_over_its_rows_with_its_summary() -> Check {
    let grouped = |id: &str, group: &str, summary: Option<&str>| {
        let mut row = service(id, Dot::Run);
        row.group = Some(group.to_owned());
        match summary {
            Some(words) => row.text("group.summary", words),
            None => row,
        }
    };
    let rows = vec![
        grouped("atlas-api", "atlas", Some("compose · 2 running")),
        grouped("atlas-db", "atlas", None),
        grouped("git", "forge", Some("compose · 1 running")),
    ];
    let mut rt = window(containers, vec![(CONTAINERS, rows)]);
    let all = rt.elements();
    let heading = keyed(&all, "containers-7.group.atlas-api")?.rect;
    let (first, second) = (
        keyed(&all, "containers-7.row.atlas-api")?.rect,
        keyed(&all, "containers-7.row.atlas-db")?.rect,
    );
    assert_eq!((heading[1], heading[3]), (first[1] + 8.0, 20.0));
    assert_eq!(first[3], 8.0 + 20.0 + 20.0, "8 above, the heading, the row");
    assert_eq!(second[3], 20.0, "a row inside a group got a heading");
    for words in [
        "atlas",
        "compose · 2 running",
        "forge",
        "compose · 1 running",
    ] {
        assert!(says(&rt, words), "the heading does not say {words:?}");
    }
    assert!(
        drawn(&rt, "containers-7.group.git"),
        "the second group has no heading"
    );
    press_at(&mut rt, "containers-7.row.atlas-api", 18.0)?;
    assert!(
        !drawn(&rt, "containers-7.detail.atlas-api"),
        "a press on a group heading opened a row"
    );
    press_at(&mut rt, "containers-7.row.atlas-api", 38.0)?;
    assert!(
        drawn(&rt, "containers-7.detail.atlas-api"),
        "the row under it"
    );
    Ok(())
}

/// A group's summary is the `group.summary` cell of its first row; with none there, the
/// heading is its name alone, whatever later rows carry.
#[test]
fn a_group_with_no_summary_on_its_first_row_shows_its_name_alone() -> Check {
    let grouped = |id: &str, summary: Option<&str>| {
        let mut row = service(id, Dot::Run);
        row.group = Some("forge".to_owned());
        match summary {
            Some(words) => row.text("group.summary", words),
            None => row,
        }
    };
    let rows = vec![
        grouped("git", None),
        grouped("ci", Some("compose · 1 running")),
    ];
    let rt = window(containers, vec![(CONTAINERS, rows)]);
    let all = rt.elements();
    let heading = keyed(&all, "containers-7.group.git")?.rect;
    let inside = |e: &&rune::ElementInfo| {
        e.text.is_some() && e.rect[1] >= heading[1] && e.rect[1] < heading[1] + heading[3]
    };
    let said: Vec<_> = all
        .iter()
        .filter(inside)
        .filter_map(|e| e.text.clone())
        .collect();
    assert_eq!(said, ["forge"], "the heading says more than its name");
    assert!(
        !says(&rt, "compose · 1 running"),
        "a later row's summary is drawn"
    );
    Ok(())
}

/// Record 5.1.4 and 1.6.8: each kind of cell at its measured size and in its role's colour.
#[test]
fn each_kind_of_cell_is_drawn_to_the_record() -> Check {
    let one = Row::new("atlas-api")
        .text("name", "atlas-api")
        .cell("state", Value::Dot(Dot::Run))
        .cell("level", Value::Badge("High".to_owned(), Tone::Bad))
        .cell("stage", Value::Pill("Proving".to_owned(), Tone::Need))
        .cell(
            "cpu",
            Value::Meter {
                ratio: 0.71,
                text: "71%".to_owned(),
            },
        )
        .cell(
            "touched",
            Value::Tags(
                ["upload.rs +41", "read b.rs", "problem from rust-analyzer"]
                    .map(str::to_owned)
                    .into(),
            ),
        )
        .cell(
            "took",
            Value::Bar {
                start: 0.5,
                len: 0.25,
                tone: Tone::Accent,
            },
        )
        .cell("login", Value::Flag(true))
        .cell("load", Value::Series(vec![1.0, 2.0, 4.0]));
    let mut dim = service("registry", Dot::Fail).cell("login", Value::Flag(false));
    dim.dim = true;
    let rt = window_of(
        containers,
        (800.0, 360.0),
        vec![(CONTAINERS, vec![one, dim])],
    );
    let (all, tokens) = (rt.elements(), Tokens::default());
    let dots = sized(&all, 7.0, 7.0);
    assert!(
        dots.iter().any(|e| e.fill == colour(tokens.ok)),
        "a running dot is green"
    );
    assert!(
        dots.iter().any(|e| e.fill == colour(tokens.bad)),
        "a failed dot is red"
    );
    let badge = boxed(&all, worded(&all, "High")?)?;
    assert_eq!(
        (badge.rect[2], badge.rect[3]),
        (32.0, 13.0),
        "a severity badge"
    );
    assert_eq!(badge.fill, colour(tokens.bad.alpha(0.22)));
    let pill = boxed(&all, worded(&all, "Proving")?)?;
    assert_eq!((pill.rect[3], pill.radius), (14.0, 7.0), "a state pill");
    assert_eq!(pill.fill, colour(tokens.need.alpha(0.2)));
    let track = sized(&all, 34.0, 4.0);
    assert_eq!(track.len(), 1, "one meter track of 34 by 4");
    let fill = all
        .iter()
        .find(|e| e.fill == colour(tokens.need) && e.rect[3] == 4.0);
    let fill = fill.ok_or("a meter at 71% is not in the need colour")?;
    assert!(
        (fill.rect[2] - 34.0 * 0.71).abs() < 0.01,
        "filled to its ratio"
    );
    assert!(says(&rt, "71%"), "the meter's reading");
    let tag = boxed(&all, worded(&all, "upload.rs +41")?)?;
    assert_eq!((tag.rect[3], tag.radius), (13.0, 3.0), "a tag");
    assert!(says(&rt, "read b.rs"), "the second tag");
    let lane = sized(&all, 60.0, 8.0);
    let lane = lane
        .first()
        .ok_or("no bar lane of the column's width, 8 tall")?;
    let bar = all
        .iter()
        .find(|e| e.fill == colour(tokens.accent) && e.rect[3] == 8.0);
    let bar = bar.ok_or("no bar in its tone")?.rect;
    assert_eq!(
        (bar[0] - lane.rect[0], bar[2]),
        (30.0, 15.0),
        "start and length"
    );
    let third = worded(&all, "problem from rust-analyzer")?.visible;
    assert!(
        third[0] + third[2] <= lane.rect[0] - 8.0 + 0.01,
        "a tag runs out of its column: {third:?}, the next starts at {}",
        lane.rect[0]
    );
    let toggles = sized(&all, 20.0, 11.0);
    assert!(
        toggles.iter().any(|e| e.fill == colour(tokens.ok)),
        "a flag that is on"
    );
    assert!(
        toggles.iter().any(|e| e.fill == Color::WHITE.alpha(0.16)),
        "a flag that is off"
    );
    let filled = sized(&all, 2.0, 9.0).into_iter().filter(|e| e.fill.a > 0.0);
    assert_eq!(filled.count(), 1, "the tallest bar of a series is 9");
    assert_eq!(
        worded(&all, "registry")?.opacity,
        0.5,
        "a stopped thing is dim"
    );
    assert_eq!(worded(&all, "atlas-api")?.opacity, 1.0);
    Ok(())
}

/// A row's level colours its words, and an error line carries its tint (record 5.5); an
/// opened line shows its raw record under its facts.
#[test]
fn a_line_takes_its_level_and_opens_to_its_raw_record() -> Check {
    let mut rows = lines(0..3);
    if let Some(error) = rows.get_mut(1) {
        error.tone = Some(Tone::Bad);
    }
    let mut rt = window(logs, vec![(LOGS, rows)]);
    let (all, tokens) = (rt.elements(), Tokens::default());
    let bad = colour(tokens.bad);
    assert_eq!(
        worded(&all, "line 1")?.text_color,
        Some(bad),
        "an error's words"
    );
    assert_eq!(worded(&all, "line 0")?.text_color, Some(colour(tokens.dim)));
    let line = keyed(&all, "logs-7.row.r1")?.rect;
    let tint = colour(tokens.bad.alpha(0.13));
    assert!(
        all.iter().any(|e| e.rect == line && e.fill == tint),
        "an error line has no tint"
    );
    press(&mut rt, "logs-7.row.r1")?;
    assert!(
        says(&rt, "{\"id\":\"r1\",\"level\":\"error\"}"),
        "the raw record is not under the facts"
    );
    Ok(())
}

/// A theme changes the values a row is drawn from; rows already on screen must follow.
#[test]
fn a_new_theme_reaches_rows_that_are_already_drawn() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    let was = worded(&rt.elements(), "a")?.text_color;
    assert_eq!(was, Some(colour(Tokens::default().fg)));
    let red = ThemeValue::Text("#ff0000".to_owned());
    rt.update(move |win, _| {
        if let Ok(mut hub) = win.hub.try_borrow_mut() {
            let _ = hub.model.tokens.set("text.1", &red);
        }
    });
    settle(&mut rt);
    let now = worded(&rt.elements(), "a")?.text_color;
    assert_eq!(
        now,
        rune::color::hex("#ff0000"),
        "a drawn row kept the old theme"
    );
    Ok(())
}

/// A value may hold any number of tags; the list draws the first of them and says how many
/// it left out, so none goes missing without a word.
#[test]
fn a_long_run_of_tags_says_how_many_it_left_out() -> Check {
    let mut rt = window(touched, vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    assert!(says(&rt, "file-23.rs"), "the twenty-fourth tag");
    assert!(!says(&rt, "file-24.rs"), "more tags than a value draws");
    assert!(says(&rt, "+6"), "nothing says six tags were left out");
    let detail = rect(&rt, "daemons-7.detail.b")?;
    let last = worded(&rt.elements(), "+6")?.rect;
    assert!(
        last[0] + last[2] <= detail[0] + detail[2] && last[1] > detail[1] + 20.0,
        "the tags of a fact do not wrap inside the detail: {last:?} in {detail:?}"
    );
    Ok(())
}

/// Record 5.2.1: a fact on a line of its own may wrap, and the buttons wrap if needed. In a
/// narrow pane nothing of a detail runs out of it.
#[test]
fn a_detail_wraps_inside_a_narrow_list() -> Check {
    let mut rt = window_of(touched, (240.0, 360.0), vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    let all = rt.elements();
    let detail = keyed(&all, "daemons-7.detail.b")?.rect;
    let sentence = worded(
        &all,
        "Write tests for the new limiter and run them all twice.",
    )?
    .rect;
    assert!(
        sentence[3] > 20.0,
        "a long value did not wrap: {sentence:?}"
    );
    let button = keyed(&all, "daemons-7.detail.b.daemons.restart")?.rect;
    let note = worded(&all, "down for 56 minutes")?.rect;
    assert!(
        note[1] >= button[1] + button[3],
        "the note did not wrap under the buttons: {note:?}, a button at {button:?}"
    );
    for (name, inside) in [("the sentence", sentence), ("the note", note)] {
        assert!(
            inside[0] >= detail[0] && inside[0] + inside[2] <= detail[0] + detail[2] - 8.0 + 0.01,
            "{name} runs out of the detail: {inside:?} in {detail:?}"
        );
    }
    Ok(())
}

/// Record 5.2.1: the facts of a detail stand three to a line, and one to a line where the
/// list is too narrow for three.
#[test]
fn the_facts_of_a_detail_stand_three_to_a_line_or_one_in_a_narrow_list() -> Check {
    // The label of a fact, not the column title that says the same: the last one drawn.
    let label = |rt: &Window, words: &str| -> Result<[f32; 4], Box<dyn Error>> {
        let all = rt.elements();
        let found = all
            .iter()
            .rfind(|e| !e.exiting && e.text.as_deref() == Some(words));
        Ok(found
            .ok_or_else(|| format!("no fact is labelled {words}"))?
            .rect)
    };
    let facts = |rt: &Window| -> Result<[[f32; 4]; 3], Box<dyn Error>> {
        Ok([label(rt, "What")?, label(rt, "PID")?, label(rt, "Up")?])
    };
    let mut wide = window(services, vec![(DAEMONS, five())]);
    press(&mut wide, &service_key("b"))?;
    let [what, pid, up] = facts(&wide)?;
    assert_eq!(
        (what[1], pid[1]),
        (up[1], up[1]),
        "three facts are not on one line"
    );
    assert!(
        what[0] < pid[0] && pid[0] < up[0],
        "the facts are out of order"
    );
    assert_eq!(
        pid[0] - what[0],
        up[0] - pid[0],
        "the columns are not equal"
    );
    let named = label(&wide, "Named")?;
    assert_eq!(named[0], what[0], "the fourth fact starts the second line");
    assert!(named[1] > what[1]);

    let mut narrow = window_of(services, (300.0, 360.0), vec![(DAEMONS, five())]);
    press(&mut narrow, &service_key("b"))?;
    let [what, pid, up] = facts(&narrow)?;
    assert_eq!(
        (pid[0], up[0]),
        (what[0], what[0]),
        "a narrow list kept its columns"
    );
    assert!(
        what[1] < pid[1] && pid[1] < up[1],
        "the facts do not stand one to a line"
    );
    Ok(())
}

/// Record 5.2.2, rule 5: the scene `just scenes` renders starts with a row open in each list.
#[test]
fn the_demo_scene_opens_a_row_in_each_of_its_lists() -> Check {
    let scene = SCENES.first().ok_or("the list engine has no scene")?;
    let hub = Hub::new(DEFS, Vec::new());
    {
        let mut hub = hub.try_borrow_mut()?;
        hub.stage = scene.stage;
        (scene.setup)(&mut hub);
    }
    let win = Win::new(hub, WindowId(0));
    let mut rt = Runtime::new(win, root, (1200.0, 750.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);
    for open in [
        "agents-0.detail.tests",
        "logs-1.detail.l10",
        "containers-2.detail.atlas-api",
        "findings-3.detail.underflow",
    ] {
        assert!(drawn(&rt, open), "{open} is not open");
    }
    for words in [
        "Run its command",
        "tests/upload.rs +41",
        "compose · 4 running",
        "rebuilt by the agent 14m ago",
        "Make it a finding",
        "Allow the test",
    ] {
        assert!(says(&rt, words), "the scene does not say {words:?}");
    }
    let tree: Vec<String> = showing(&rt, "agents-0")
        .into_iter()
        .map(|row| row.0)
        .collect();
    let bisect = tree.iter().position(|key| key.ends_with(".bisect"));
    assert_eq!(bisect, Some(3), "the sample tree is out of order: {tree:?}");
    Ok(())
}
