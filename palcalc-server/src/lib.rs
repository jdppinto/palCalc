//! palcalc-server library surface.
//!
//! The server binary (`src/main.rs`) is built from these modules; exposing
//! them as a lib also lets the pinning client and integration tests reuse
//! them. See each module and `docs/CLIENT-PINNING.md` for the security model.

pub mod auth;
pub mod cache;
pub mod config;
pub mod ratelimit;
pub mod tls;

/// Certificate-pinning HTTPS client used by palCalc clients to talk to this
/// server safely over a self-signed cert. Enabled by the `client` feature.
#[cfg(feature = "client")]
pub mod pin;
