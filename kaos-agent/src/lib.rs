//! The agent runtime: how Kaos actually talks to a model.
//!
//! Providers and backends, the conductor's tool-using loop, the familiar, and
//! the pause/resume machinery. This is everything that fires a prompt, spawns a
//! child, or reaches the network — deliberately separated from the Pact, which
//! is pure, and from the front-ends, which only display what happens here.
//!
//! Nothing in this crate knows about a terminal or a window.

pub mod agent;
pub mod auth;
pub mod backend;
pub mod conductor;
pub mod familiar;
#[cfg(feature = "api")]
pub mod hand;
pub mod myth;
pub mod pause;
pub mod provider;
pub mod scry;
pub mod solve;
pub mod spiral;
