//! Versioned Unix-socket IPC protocol between `argus` and `argusd`.
//!
//! See `specs/001-bootstrap/contracts/ipc-protocol.md` for the wire contract.
//! Frames are newline-delimited JSON; identity is derived from `SO_PEERCRED`
//! (the client-supplied identity is never trusted).

mod client;
mod protocol;
mod server;

pub use client::Client;
pub use protocol::{
    DecodeError, ErrorBody, ErrorCode, Operation, PROTOCOL_VERSION, Request, Response,
    decode_request,
};
pub use server::serve;
