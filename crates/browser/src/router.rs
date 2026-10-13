//! The CDP conversation as a state machine over messages: no process, no clock, so a recorded
//! pipe transcript replays through it exactly as Chrome's live output does.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use yi_types::cdp::{
    CdpCommand, CdpMessage, KeyInput, MouseInput, NavigationHistory, ScreencastFrame,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// A screencast frame, base64 PNG as Chrome sent it.
    Frame(String),
    Url(String),
    /// A navigation Chrome could not finish; the page stays usable.
    NavigateFailed(String),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waiting {
    Target,
    Attach,
    Navigate,
    History(i64),
    Other,
}

pub struct Router {
    next: u64,
    waiting: BTreeMap<u64, (Waiting, String)>,
    session: Option<String>,
    queued: Vec<CdpCommand>,
    size: (u32, u32),
}

impl Router {
    /// Opens `url` in a new page target; frames start once it is attached.
    pub fn new(url: &str, size: (u32, u32)) -> Self {
        let mut router = Self {
            next: 0,
            waiting: BTreeMap::new(),
            session: None,
            queued: Vec::new(),
            size,
        };
        router.call(
            Waiting::Target,
            "Target.createTarget",
            json!({ "url": url }),
        );
        router
    }

    pub fn take(&mut self) -> Vec<CdpCommand> {
        std::mem::take(&mut self.queued)
    }

    fn call(&mut self, waiting: Waiting, method: &str, params: Value) {
        self.next = self.next.saturating_add(1);
        self.waiting.insert(self.next, (waiting, method.to_owned()));
        self.queued.push(CdpCommand {
            id: self.next,
            method: method.to_owned(),
            params,
            session_id: self.session.clone(),
        });
    }

    pub fn feed(&mut self, message: CdpMessage) -> Option<Update> {
        if let Some(id) = message.id {
            let (waiting, method) = self.waiting.remove(&id)?;
            if let Some(error) = message.error {
                return Some(Update::Failed(format!("{method}: {}", error.message)));
            }
            return self.answered(waiting, &message.result);
        }
        match message.method.as_deref() {
            Some("Page.screencastFrame") => {
                let frame: ScreencastFrame = serde_json::from_value(message.params).ok()?;
                let ack = json!({ "sessionId": frame.session_id });
                self.call(Waiting::Other, "Page.screencastFrameAck", ack);
                Some(Update::Frame(frame.data))
            }
            Some("Page.frameNavigated") => {
                let frame = message.params.get("frame")?;
                if frame.get("parentId").is_some() {
                    return None;
                }
                let url = frame.get("url")?.as_str()?.to_owned();
                self.cast();
                Some(Update::Url(url))
            }
            Some("Target.detachedFromTarget" | "Inspector.detached") => {
                Some(Update::Failed("the page closed".to_owned()))
            }
            _ => None,
        }
    }

    fn answered(&mut self, waiting: Waiting, result: &Value) -> Option<Update> {
        match waiting {
            Waiting::Target => {
                let target = result.get("targetId")?.clone();
                let params = json!({ "targetId": target, "flatten": true });
                self.call(Waiting::Attach, "Target.attachToTarget", params);
            }
            Waiting::Attach => {
                self.session = Some(result.get("sessionId")?.as_str()?.to_owned());
                self.call(Waiting::Other, "Page.enable", json!({}));
                self.resize(self.size);
            }
            Waiting::Navigate => {
                let error = result.get("errorText").and_then(Value::as_str);
                return error
                    .filter(|text| !text.is_empty())
                    .map(|text| Update::NavigateFailed(text.to_owned()));
            }
            Waiting::History(step) => {
                let history: NavigationHistory = serde_json::from_value(result.clone()).ok()?;
                let at = usize::try_from(history.current_index.checked_add(step)?).ok()?;
                let entry = history.entries.get(at)?.id;
                let params = json!({ "entryId": entry });
                self.call(Waiting::Other, "Page.navigateToHistoryEntry", params);
            }
            Waiting::Other => {}
        }
        None
    }

    /// The page's viewport and the screencast's frame size, both in CSS pixels.
    pub fn resize(&mut self, (width, height): (u32, u32)) {
        self.size = (width, height);
        if self.session.is_none() {
            return;
        }
        let metrics = json!({
            "width": width, "height": height, "deviceScaleFactor": 1, "mobile": false,
        });
        self.call(
            Waiting::Other,
            "Emulation.setDeviceMetricsOverride",
            metrics,
        );
        self.cast();
    }

    /// Incident: after a cross-document navigation headless Chrome sent no frame until the
    /// screencast restarted, and it refuses a start while one runs, so a start follows a stop.
    fn cast(&mut self) {
        let (width, height) = self.size;
        self.call(Waiting::Other, "Page.stopScreencast", json!({}));
        let cast = json!({
            "format": "png", "maxWidth": width, "maxHeight": height, "everyNthFrame": 1,
        });
        self.call(Waiting::Other, "Page.startScreencast", cast);
    }

    /// Invariant: Chrome rejects a page command with no session, so input before the attach
    /// answer is dropped rather than sent.
    fn page_call(&mut self, waiting: Waiting, method: &str, params: Value) {
        if self.session.is_some() {
            self.call(waiting, method, params);
        }
    }

    pub fn navigate(&mut self, url: &str) {
        self.page_call(Waiting::Navigate, "Page.navigate", json!({ "url": url }));
    }

    /// Back is -1 and forward +1; a step past either end of the history does nothing.
    pub fn go(&mut self, step: i64) {
        self.page_call(
            Waiting::History(step),
            "Page.getNavigationHistory",
            json!({}),
        );
    }

    pub fn reload(&mut self) {
        self.page_call(Waiting::Other, "Page.reload", json!({}));
    }

    pub fn mouse(&mut self, input: &MouseInput) {
        let params = serde_json::to_value(input).unwrap_or_default();
        self.page_call(Waiting::Other, "Input.dispatchMouseEvent", params);
    }

    pub fn key(&mut self, input: &KeyInput) {
        let params = serde_json::to_value(input).unwrap_or_default();
        self.page_call(Waiting::Other, "Input.dispatchKeyEvent", params);
    }

    pub fn insert_text(&mut self, text: &str) {
        self.page_call(Waiting::Other, "Input.insertText", json!({ "text": text }));
    }
}
