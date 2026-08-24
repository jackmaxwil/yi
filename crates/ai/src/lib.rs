#![forbid(unsafe_code)]

pub mod anthropic;
pub mod auth;
pub mod catalog;
pub mod faux;
pub mod json_salvage;
pub mod openai;
pub mod request;
pub mod retry;
pub mod sse;
pub mod transform;

pub(crate) use yi_types::event::AssistantMessageEvent as EventOut;
