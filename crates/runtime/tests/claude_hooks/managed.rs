use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_runtime::claude_hooks::{Call, Event, Hooks};

use super::{Case, TestResult, settings, touching, write_json};

fn ran(case: &Case, managed: &std::path::Path) -> Map<String, Value> {
    let hooks = Hooks::load_from(&case.cwd, Some(&case.home), managed);
    let call = Call {
        tool: "bash".to_owned(),
        args: Map::from_iter([("command".to_owned(), json!("printf ran"))]),
        cwd: case.cwd.clone(),
        ..Call::default()
    };
    let never: yi_tools::CancelFlag = Arc::new(|| false);
    let _outcome = hooks.run(Event::PreToolUse, &call, &never);
    let mut seen = Map::new();
    for name in ["managed", "drop-in", "user"] {
        seen.insert(name.to_owned(), Value::Bool(case.cwd.join(name).exists()));
    }
    seen
}

#[test]
fn managed_policy_merges_its_drop_ins_and_can_keep_only_its_own_hooks() -> TestResult {
    let case = Case::new("managed", json!({}), true)?;
    let managed = case.path("managed");
    let hook = |name: &str| settings("PreToolUse", "*", vec![touching(&case.cwd.join(name))]);
    case.user(&hook("user"))?;
    let mut policy = hook("managed");
    write_json(&managed.join("managed-settings.json"), &policy)?;
    write_json(
        &managed.join("managed-settings.d/10-extra.json"),
        &hook("drop-in"),
    )?;
    let all = ran(&case, &managed);
    assert_eq!(
        all,
        json!({"managed": true, "drop-in": true, "user": true})
            .as_object()
            .cloned()
            .unwrap_or_default()
    );
    for name in ["managed", "drop-in", "user"] {
        std::fs::remove_file(case.cwd.join(name))?;
    }
    policy["allowManagedHooksOnly"] = Value::Bool(true);
    write_json(&managed.join("managed-settings.json"), &policy)?;
    let only = ran(&case, &managed);
    assert_eq!(only["user"], false, "allowManagedHooksOnly ran a user hook");
    assert_eq!(only["managed"], true);
    std::fs::remove_file(case.cwd.join("managed"))?;
    std::fs::remove_file(case.cwd.join("drop-in"))?;
    policy["disableAllHooks"] = Value::Bool(true);
    write_json(&managed.join("managed-settings.json"), &policy)?;
    let none = ran(&case, &managed);
    assert!(
        none.values().all(|ran| ran == &Value::Bool(false)),
        "{none:?}"
    );
    Ok(())
}
