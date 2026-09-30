use serde_json::{Map, Value};

pub(crate) trait Args {
    fn str_of(&self, key: &str) -> Option<&str>;
    fn u64_of(&self, key: &str) -> Option<u64>;
    fn bool_of(&self, key: &str) -> Option<bool>;
}

impl Args for Map<String, Value> {
    fn str_of(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    fn u64_of(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(Value::as_u64)
    }

    fn bool_of(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }
}
