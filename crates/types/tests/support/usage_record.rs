//! A finished child's usage record in the shape the runtime's attribution writes it, for tests
//! that seed a ledger with child spend.
use yi_types::record::LaneRecord;

pub fn child_usage(id: String, tokens: i64, cost: f64) -> Result<LaneRecord, serde_json::Error> {
    serde_json::from_value(
        serde_json::json!({"type": "usage", "id": id, "lane": "main",
        "cause": "child_usage_attributed", "seq": 0, "timestamp": 0,
        "usage": {"input": tokens, "output": 0, "cacheRead": 0, "cacheWrite": 0,
            "totalTokens": tokens, "cost": {"input": 0, "output": 0, "cacheRead": 0,
            "cacheWrite": 0, "total": cost}}}),
    )
}
