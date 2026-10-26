//! Chrome DevTools Protocol over `--remote-debugging-pipe`: one JSON object per frame, each
//! frame ended by a NUL byte. Only the methods the console's browser pane speaks are typed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `sessionId` routes a command to an attached target; absent, the browser itself answers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CdpCommand {
    pub id: u64,
    pub method: String,
    pub params: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// What Chrome writes: a response carries the command's `id`, an event a `method`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CdpMessage {
    pub id: Option<u64>,
    pub method: Option<String>,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub result: Value,
    pub error: Option<CdpError>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
}

/// `Page.screencastFrame`: `data` is base64 PNG, and `sessionId` is the number its ack echoes,
/// not a target session.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreencastFrame {
    pub data: String,
    pub session_id: i64,
}

/// `Page.getNavigationHistory`'s result.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigationHistory {
    pub current_index: i64,
    pub entries: Vec<NavigationEntry>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NavigationEntry {
    pub id: i64,
    pub url: String,
}

/// `Input.dispatchMouseEvent`'s params; `x` and `y` are CSS pixels from the viewport's corner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MouseInput {
    #[serde(rename = "type")]
    pub kind: MouseKind,
    pub x: u32,
    pub y: u32,
    pub button: MouseButton,
    pub buttons: u8,
    pub click_count: u8,
    pub delta_x: i32,
    pub delta_y: i32,
    pub modifiers: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseKind {
    MousePressed,
    MouseReleased,
    MouseMoved,
    MouseWheel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    None,
    Left,
}

/// `Input.dispatchKeyEvent`'s params; a `keyDown` with `text` also types it. `modifiers` is
/// the CDP bitmask: Alt 1, Ctrl 2, Meta 4, Shift 8.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInput {
    #[serde(rename = "type")]
    pub kind: KeyKind,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub windows_virtual_key_code: u32,
    pub modifiers: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyKind {
    KeyDown,
    KeyUp,
}
