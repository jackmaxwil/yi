//! The plan engine writes the vectors the Python fake host (`python/yi_runtime/tests/fake_host.py`)
//! reads, so the fake cannot keep a table the engine has since changed.

use std::error::Error;
use std::path::Path;

use serde_json::{Value, json};
use yi_runtime::plan::table::{RETRY_CAP, STEPS, op_name};
use yi_types::plan::doc::TODO_LABEL_MAX;

fn engine_vectors() -> Value {
    let mut legal = serde_json::Map::new();
    for step in STEPS {
        if let Value::Array(from) = legal.entry(op_name(step.op)).or_insert_with(|| json!([])) {
            from.push(json!(step.from.as_str()));
        }
    }
    json!({"label_max": TODO_LABEL_MAX, "retry_cap": RETRY_CAP.0, "legal_from": legal})
}

#[test]
fn the_committed_plan_vectors_are_what_the_engine_writes() -> Result<(), Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../python/yi_runtime/tests/vectors/plan_steps.json");
    let fresh = format!("{}\n", serde_json::to_string_pretty(&engine_vectors())?);
    if std::env::var_os("YI_BLESS").is_some() {
        std::fs::write(&path, &fresh)?;
    }
    assert_eq!(
        std::fs::read_to_string(&path)?,
        fresh,
        "plan_steps.json is stale; run `YI_BLESS=1 cargo test -p yi-runtime --test integration plan_vectors`"
    );
    Ok(())
}
