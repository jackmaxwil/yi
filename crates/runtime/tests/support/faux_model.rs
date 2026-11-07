//! The faux provider's model: a session that boots and never reaches a network.
use yi_types::model::{Model, ModelCost};

pub fn faux_model(context_window: u64) -> Model {
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost::free(),
        context_window,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
        read_timeout_secs: None,
    }
}
