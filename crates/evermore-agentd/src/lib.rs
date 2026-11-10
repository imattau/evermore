//! Bounded, deterministic agent components used by the Phase 11 daemon:
//! recipes, task state, catalogue reads, and constrained L1 proposals. The
//! executable in `main.rs` wires them to the Unix-socket protocol.

pub mod agent_runtime;
pub mod agents;
pub mod clients;
pub mod infer;
pub mod mcp_agent;
pub mod nscript_recipe;
pub mod propose;
pub mod recipe;
pub mod relay_client;
pub mod render;
pub mod routine;
pub mod runner;
pub mod scheduler;
pub mod store;
