//! The dependency-free heart of Kaos.
//!
//! Everything here is pure and std-only (bar the Rebis parser the mandala
//! model needs): no terminal, no window, no rendering, no network. That is the
//! point of the split — both front-ends, the ratatui terminal app and the egui
//! visual editor, are thin shells over this crate, and every rule they share
//! lives here once and is tested without either of them on screen.
//!
//! - [`config`] — the persistent, non-secret settings file.
//! - [`theme`] — the monochrome palette and its two modes.
//! - [`sessions`] — durable chat transcripts.
//! - [`tabs`] — an ordered set of tabs, generic over what they hold.
//! - [`visual`] — the mandala model, Rebis code generation and loading.

pub mod config;
pub mod sessions;
pub mod tabs;
pub mod theme;
pub mod visual;
