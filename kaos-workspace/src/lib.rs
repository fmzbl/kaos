//! The Rebis workspace: everything the editor *is*, with nothing that draws it.
//!
//! Buffer and cursor, Vim modes and motions, the command palette, the sigil
//! explorer, run state and checkpoints. It never paints: commands return a
//! [`rebis_workspace::WorkspaceAction`] and the caller decides what that looks
//! like on screen.
//!
//! That is what lets a terminal and a window drive the same editor rather than
//! each growing its own.

pub mod rebis_checkpoint;
pub mod rebis_supervisor;
pub mod rebis_workspace;
