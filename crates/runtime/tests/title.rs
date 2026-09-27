//! A session's model-written title, cleaned to a short plain line.
use yi_runtime::title::clean;

/// Dies with the model's reply used verbatim: quotes, a heading mark, a trailing period and a
/// second line all reached the sidebar as the session's name.
#[test]
fn a_title_is_one_short_plain_line() {
    assert_eq!(
        clean("\"Fix the console context gauge.\"\nBecause the figure was wrong."),
        Some("Fix the console context gauge".to_owned())
    );
    assert_eq!(clean("# Orb states\n"), Some("Orb states".to_owned()));
    assert_eq!(clean("   \n  "), None);
    let long = clean(&"word ".repeat(30)).unwrap_or_default();
    assert!(long.chars().count() <= 48, "{long}");
    assert!(long.ends_with("word"), "cut at a word: {long}");
}
