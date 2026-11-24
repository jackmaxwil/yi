//! A feed that carries several kinds of row, read one kind at a time.

use yd_core::feed::{FeedEvent, FeedStatus, Table};
use yd_core::ids::RowId;
use yd_core::row::{Row, Rows};

type Check = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_prefix_gives_only_that_kind_of_row_and_holds_its_version_until_one_changes() -> Check {
    let mut feed = Table::default();
    feed.apply(FeedEvent::Reset(vec![
        Row::new("var:x").text("value", "1"),
        Row::new("frame:0").text("name", "main"),
        Row::new("var:y").text("value", "2"),
    ]));
    feed.apply(FeedEvent::Status(FeedStatus::Live));

    let vars = feed.prefixed("var:");
    assert_eq!(vars.len(), 2);
    assert!(vars.find(&RowId::new("frame:0")).is_none());
    assert_eq!(vars.status(), &FeedStatus::Live);
    assert_eq!(vars.version(), feed.prefixed("var:").version());

    feed.apply(FeedEvent::Upsert(Row::new("frame:0").text("name", "take")));
    assert_eq!(vars.version(), feed.prefixed("var:").version());
    feed.apply(FeedEvent::Upsert(Row::new("var:x").text("value", "9")));
    assert_ne!(vars.version(), feed.prefixed("var:").version());
    Ok(())
}
