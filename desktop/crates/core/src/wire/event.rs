//! What the client's threads hand the UI: one decoded frame, or the socket's fate.

use crate::ids::Millis;
use crate::wire::frame::Frame;
use crate::wire::request::RequestId;

#[derive(Debug, Clone, PartialEq)]
pub enum WireEvent {
    /// One line from the server, not yet decoded: how a recording or a scene feeds the model.
    Line(String),
    /// One frame, decoded on the reader thread and stamped when it arrived.
    Frame { at: Millis, frame: Box<Frame> },
    /// The socket is open and the server answered `initialize`.
    Up {
        at: Millis,
        server: String,
        /// How long the server took to answer `initialize`; none when nothing timed it.
        latency: Option<Millis>,
    },
    /// The socket closed or could not be opened; the client keeps trying.
    Down { at: Millis, why: Down },
    /// A request the client could not write: it never left this app.
    Unsent { at: Millis, id: RequestId },
    /// The client's timer: time to ask for the thread list again.
    Poll { at: Millis },
    /// Bytes on the socket that were not a frame.
    Bad { at: Millis, why: String },
}

/// Why there is no connection to the server.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Down {
    #[error("no server is listening on the socket")]
    NoServer,
    #[error("the server was stopped on purpose, so it is not started again")]
    Stopped,
    #[error("the server could not be started: {0}")]
    StartFailed(String),
    #[error("the server did not answer initialize: {0}")]
    Handshake(String),
    #[error("the socket closed: {0}")]
    Closed(String),
}
