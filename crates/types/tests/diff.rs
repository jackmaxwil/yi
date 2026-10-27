use std::error::Error;

#[test]
fn changed_edges_leave_only_the_unshared_middle() -> Result<(), Box<dyn Error>> {
    let check = |before: &[&str], after: &[&str], expected| {
        assert_eq!(yi_types::diff::changed_edges(before, after), expected);
    };
    check(&[], &[], (0, 0));
    check(&["same\n"], &["same\n"], (1, 0));
    check(&["a\n", "b\n"], &["a\n", "c\n"], (1, 0));
    check(&["a\n", "b\n", "z\n"], &["a\n", "c\n", "z\n"], (1, 1));
    check(&["a\n"], &["a\n", "b\n"], (1, 0));
    Ok(())
}
