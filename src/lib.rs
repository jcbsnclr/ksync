//! ksync v2: a content-addressed, chunked sync store.
//!
//! This crate is split into a library and a binary so integration tests can
//! drive the listener in-process.

pub mod auth;
pub mod client;
pub mod config;
pub mod control;
pub mod daemon;
pub mod identity;
pub mod method;
pub mod methods;
pub mod proto;
pub mod server;
pub mod shutdown;
pub mod tls;
