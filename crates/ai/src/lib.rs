#![forbid(unsafe_code)]

pub mod anthropic;
pub mod auth;
pub mod breakpoints;
pub mod catalog;
pub mod compat;
pub mod decide;
pub mod faux;
pub use yi_types::json_salvage;
pub mod leak;
pub mod openai;
pub mod openai_responses;
pub mod refresh;
pub mod request;
pub mod retry;
pub mod settle;
pub mod sse;
pub mod transform;

pub(crate) use yi_types::event::AssistantMessageEvent as EventOut;
