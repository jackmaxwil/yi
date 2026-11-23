//! The tree assertions' own tests, on a small window drawn in process.
#![allow(dead_code, reason = "the module is shared with other test binaries")]

mod tree;

use rune::{El, Env, Runtime, col, row, text};

use tree::{
    Check, assert_golden, assert_row, assert_size, by_text, compare, rect, render, row_text,
};

struct Shown;

fn view(_: &Shown, _: &mut rune::Cx<Shown>) -> El {
    col()
        .key("list")
        .child(
            row()
                .key("list.row.a")
                .h(20.0)
                .gap(8.0)
                .child(text("pid 41").absolute(300.0, 0.0).size(60.0, 20.0))
                .child(text("com.yi.daemon"))
                .child(text("Running — ✓")),
        )
        .child(
            row()
                .key("list.row.b")
                .h(20.0)
                .child(text("homebrew.mxcl.postgresql@16")),
        )
}

fn drawn() -> Vec<rune::ElementInfo> {
    let mut runtime = Runtime::new(Shown, view, (400.0, 120.0), Env::new(true, 1.0));
    runtime.frame(1.0 / 60.0);
    runtime.elements()
}

#[test]
fn a_row_reads_its_own_words_left_to_right() -> Check {
    let tree = drawn();
    assert_row(&tree, "list.row.a", "com.yi.daemon Running — ✓ pid 41")?;
    assert_eq!(
        row_text(&tree, "list.row.b")?,
        "homebrew.mxcl.postgresql@16"
    );
    Ok(())
}

#[test]
fn a_wrong_row_fails_with_both_texts() -> Check {
    let tree = drawn();
    let error = assert_row(&tree, "list.row.a", "com.yi.daemon Stopped")
        .err()
        .ok_or("a wrong row passed")?;
    assert!(error.to_string().contains("Running — ✓"), "{error}");
    Ok(())
}

#[test]
fn a_missing_key_names_the_key() -> Check {
    let error = rect(&drawn(), "list.row.zz")
        .err()
        .ok_or("found a missing key")?;
    assert!(error.to_string().contains("list.row.zz"), "{error}");
    Ok(())
}

#[test]
fn a_size_passes_within_its_tolerance_only() -> Check {
    let tree = drawn();
    assert_size(&tree, "list.row.a", (400.0, 20.0), 0.5)?;
    assert!(assert_size(&tree, "list.row.a", (400.0, 21.0), 0.5).is_err());
    assert!(by_text(&tree, "pid 41")?.rect[2] > 0.0);
    Ok(())
}

#[test]
fn a_tree_matches_its_golden_and_a_change_does_not() -> Check {
    let mut tree = drawn();
    assert_golden(&tree, "tests/support/golden/rows.txt")?;
    if let Some(word) = tree
        .iter_mut()
        .find(|e| e.text.as_deref() == Some("pid 41"))
    {
        word.text = Some("pid 42".into());
    }
    let error = compare(&render(&tree), "tests/support/golden/rows.txt", false)
        .err()
        .ok_or("a changed tree matched")?;
    assert!(error.to_string().contains("line"), "{error}");
    Ok(())
}
