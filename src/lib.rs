//! ksync v2: a content-addressed, chunked sync store.
//!
//! This crate is split into a library and a binary so integration tests can
//! drive the listener in-process.

pub mod auth;
pub mod client;
pub mod identity;
pub mod proto;
pub mod server;
pub mod tls;
