use serde_json::{Value, json};
use yi_types::config::RewriteLength;

use crate::{AcpState, INVALID_PARAMS, respond};

impl AcpState {
    /// `_yi/rewrite`: answered from a task when the model has, with `{"text"}` or `{"reason"}`
    /// (a rewrite that cannot run is no protocol error); only bad params answer here.
    pub(crate) fn rewrite(
        &mut self,
        request: Value,
        params: &Value,
    ) -> Option<Result<Value, (i64, String)>> {
        if let Ok(id) = Self::session_id(params) {
            self.ensure_loaded(&id);
        }
        let (handle, _) = match self.session(params) {
            Ok(found) => found,
            Err(error) => return Some(Err(error)),
        };
        let length = match params
            .get("length")
            .cloned()
            .map(serde_json::from_value::<RewriteLength>)
        {
            Some(Ok(length)) => length,
            _ => {
                return Some(Err((
                    INVALID_PARAMS,
                    "length is short, medium or long".to_owned(),
                )));
            }
        };
        let Some(draft) = params.get("draft").and_then(Value::as_str) else {
            return Some(Err((
                INVALID_PARAMS,
                "draft is the text to rewrite".to_owned(),
            )));
        };
        let draft = draft.to_owned();
        let session = std::sync::Arc::clone(&handle.session);
        let sink = std::sync::Arc::clone(&self.sink);
        tokio::spawn(async move {
            let answer = match session.rewrite_draft(length, &draft).await {
                Ok(text) => json!({"text": text}),
                Err(reason) => json!({"reason": reason}),
            };
            respond(&sink, request, Ok(answer));
        });
        None
    }
}
