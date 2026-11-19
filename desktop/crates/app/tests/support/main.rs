//! The app's test support, run against a registered scene.
#![allow(dead_code, reason = "the modules are shared with other test binaries")]

mod scene;
#[path = "../../../ui/tests/support/tree.rs"]
mod tree;

use tree::{Check, assert_size, by_key};

#[test]
fn a_scene_draws_the_window_and_its_pane() -> Check {
    let tree = scene::elements("daemons/sample")?;
    assert_size(&tree, "root", scene::WINDOW, 0.0)?;
    by_key(&tree, "daemons-0")?;
    Ok(())
}

#[test]
fn an_unknown_scene_is_an_error_naming_it() -> Check {
    let error = scene::elements("daemons/none")
        .err()
        .ok_or("drew a scene that is not registered")?;
    assert!(error.to_string().contains("daemons/none"), "{error}");
    Ok(())
}
