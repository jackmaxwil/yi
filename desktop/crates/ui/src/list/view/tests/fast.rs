//! Every list takes the fast path: rows that did not move are not read again, whether the
//! table is a feed's own, an instance of a feed, or one a pane made in its view.

use rune::{Cx, El};
use yd_core::feed::{FeedEvent, Table};
use yd_core::ids::Millis;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::{Dot, Row};

use super::{
    AGENTS, Check, DAEMONS, PROBLEMS, SERVERS, SERVICES, agents, five, list, pane, says, send,
    service, services, showing, two, window,
};
use crate::list::view::KEYS;
use crate::win::{Hub, Win};

/// Rows a pane makes in its view with the same ids each run; their words carry the clock.
fn remade(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let now = hub.model.now.0;
    let mut table = Table::default();
    table.apply(FeedEvent::Reset(
        ["a", "b", "c"]
            .map(|id| service(id, Dot::Run).text("name", format!("{id} at {now}")))
            .into(),
    ));
    list(&pane(hub), PaneKind::Daemons, &SERVICES, Some(&table))
}

#[test]
fn a_list_over_an_instance_of_a_feed_reads_no_keys_when_the_view_runs_again() -> Check {
    let mut rt = window(two, vec![(SERVERS, five()), (PROBLEMS, five())]);
    let before = KEYS.get();
    send(&mut rt, Msg::Tick(Millis(1)));
    assert_eq!(KEYS.get(), before, "a view run read the row keys again");
    Ok(())
}

#[test]
fn rows_made_in_the_view_read_no_keys_and_still_show_new_words() -> Check {
    let mut rt = window(remade, Vec::new());
    assert!(says(&rt, "b at 0"));
    let before = KEYS.get();
    send(&mut rt, Msg::Tick(Millis(1)));
    assert_eq!(KEYS.get(), before, "the same ids were read again");
    assert!(says(&rt, "b at 1"), "a kept row shows its old words");
    Ok(())
}

#[test]
fn a_row_updated_in_place_reads_no_keys() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    let before = KEYS.get();
    let renamed = service("c", Dot::Fail).text("name", "c, restarted");
    send(&mut rt, Msg::Feed(DAEMONS, FeedEvent::Upsert(renamed)));
    assert_eq!(KEYS.get(), before, "an update in place read every key");
    assert!(says(&rt, "c, restarted"));
    Ok(())
}

#[test]
fn a_row_given_a_new_parent_moves_under_it() -> Check {
    let rows = vec![Row::new("root"), Row::new("one"), Row::new("two")];
    let mut rt = window(agents, vec![(AGENTS, rows)]);
    let mut moved = Row::new("one");
    moved.parent = Some(yd_core::ids::RowId::new("two"));
    send(&mut rt, Msg::Feed(AGENTS, FeedEvent::Upsert(moved)));
    let order: Vec<String> = showing(&rt, "agents-7").into_iter().map(|r| r.0).collect();
    assert_eq!(
        order,
        ["agents-7.row.root", "agents-7.row.two", "agents-7.row.one"],
        "the tree kept its old order"
    );
    Ok(())
}
