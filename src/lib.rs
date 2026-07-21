//! # kaos
//!
//! An agent orchestrator dressed as a chaos-magick secret society — and an
//! a terminal app to drive it.
//!
//! The thesis, in one line: **the techniques of chaos magick are a working
//! vocabulary for prompt engineering, and Carroll's First Equation of Magic is a
//! literal objective function for an agent orchestrator.**
//!
//! ```text
//!     M = G · L · (1 − A) · (1 − R)              (Liber Kaos)
//! ```
//!
//! - A **sigil** ([`sigil`]) is intent compression + context isolation. It depresses
//!   the *awareness* factor **A** — and the amount is computed from the real
//!   Spare/word-method compression, so the mysticism drives the number.
//! - **Banishing** (a context reset) depresses *resistance* **R** — it stops the rot
//!   of a failed attempt from poisoning the next.
//! - The **eight rays** ([`ray`]) are a mixture-of-experts router; matching the ray
//!   raises *gnosis* **G** through a competence match.
//! - The **Pact** ([`order`]) is the society of
//!   agents, with **grades** ([`grade`]) that float online (a bandit in robes).
//! - The **egregore** ([`egregore`]) is the Pact's shared memory; it raises the
//!   *link* **L** so the orchestrator improves across a session.
//! - A **rite** ([`rite`]) is the orchestration loop that performs one task.
//!
//! Benchmarks are NOT built in — kaos is a general agent. A bench is an external
//! driver over the general commands (`code`, `conclave`, `cast`), supplying its
//! own dataset and gate. See `docs/EDGE.md` for the measured results.
//!
//! The simulation core is zero-dependency, offline, and deterministic. The live
//! [`backend`] shells out to the authed `claude` CLI to fire a charged sigil for
//! real, without changing any of the orchestration above.

// The front-end-agnostic core lives in its own crate. Re-exported so the app's
// `crate::config` / `crate::theme` / `crate::sessions` / `crate::visual` paths
// keep meaning what they always did.
pub use kaos_core::{config, sessions, sigils, tabs, theme, visual};

pub mod adept;
pub mod agent;
pub mod auth;
pub mod backend;
pub mod charge;
pub mod conductor;
pub mod dream;
pub mod egregore;
pub mod equation;
pub mod familiar;
pub mod fold;
pub mod gnosis;
pub mod grade;
#[cfg(feature = "api")]
pub mod hand;
pub mod input;
pub mod myth;
pub mod order;
pub mod pause;
pub mod provider;
pub mod ray;
pub mod rebis_checkpoint;
pub mod rebis_supervisor;
pub mod rebis_workspace;
pub mod rite;
pub mod rng;
pub mod scry;
pub mod sigil;
pub mod solve;
pub mod spiral;
#[cfg(feature = "tui")]
pub mod tui;
#[cfg(feature = "visual")]
pub mod visual_ui;
pub mod working;
