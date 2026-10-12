use serde_json::Value;

pub fn current_value(reply: &Value, config_id: &str) -> Option<String> {
    reply["result"]["configOptions"]
        .as_array()?
        .iter()
        .find(|option| option["configId"] == config_id)?["currentValue"]
        .as_str()
        .map(str::to_owned)
}
