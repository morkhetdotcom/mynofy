#![forbid(unsafe_code)]
//! RAM topic owner, configuration, ACL and server transports.
pub use mynofy_protocol::{self as protocol, BoxError, crypto};
pub mod config;
pub mod engine;
pub mod transport;

mod ids;

mod cursor;

mod storage;

mod expiration;
mod ordered;

mod acl;

mod direct;
pub mod output;

pub mod admin;

mod security;

/// Binary message-storage blocks shared between RAM and on-disk images.
pub mod message_image {
    pub use crate::storage::image::{ImageLimits, MessageImage};
}

/// Optional startup-only content restore and asynchronous snapshots.
pub mod content;

#[cfg(test)]
mod test_support;

mod links;
