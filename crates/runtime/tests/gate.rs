//! Holds compile down for a child that cannot reach a human (plan section 7.6, D215).

use yi_permission::Decision;
use yi_runtime::gate::compile_ask;

fn ask() -> Decision {
    Decision::Ask {
        title: "bash requires permission".to_owned(),
        description: "deletes outside the workspace: rm -rf ../shared".to_owned(),
        reviewable: true,
    }
}

/// Dies with `compile_ask`: leave a detached child's `Ask` a question and it waits on a surface
/// nobody is at. An attached child's question stands, queued on the broker its family shares.
#[test]
fn a_detached_childs_ask_compiles_to_deny_and_an_attached_ones_stays_a_question()
-> Result<(), String> {
    let Decision::Deny { reason } = compile_ask(ask(), false) else {
        return Err("a detached child's ask must be denied".to_owned());
    };
    assert!(
        reason.contains("rm -rf ../shared"),
        "the hazard is named: {reason}"
    );
    assert!(reason.contains("no interactive surface"), "{reason}");
    assert_eq!(compile_ask(ask(), true), ask());
    let allow = Decision::Allow {
        reason: "read only".to_owned(),
    };
    assert_eq!(
        compile_ask(allow.clone(), false),
        allow,
        "only a question compiles"
    );
    Ok(())
}
