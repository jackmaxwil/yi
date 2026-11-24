//! The wire client: reader and writer threads on the socket to `yi serve`, replay, and the
//! frames in both directions.

pub mod client;
pub mod event;
pub mod frame;
pub mod replay;
pub mod request;
pub mod shapes;
