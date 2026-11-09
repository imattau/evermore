//! Bounded, deterministic agent components used by the Phase 11 daemon:
//! recipes, task state, catalogue reads, and constrained L1 proposals. The
//! executable in `main.rs` wires them to the Unix-socket protocol.

pub mod clients;
pub mod infer;
pub mod nscript_recipe;
pub mod propose;
pub mod recipe;
pub mod render;
pub mod routine;
pub mod runner;
pub mod store;
