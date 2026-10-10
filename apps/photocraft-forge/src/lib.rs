//! Forge — the PhotoCraft development MCP.
//!
//! Forge is an **AI-first development control plane** for PhotoCraft: it gives any MCP-capable AI
//! the must-change-together touch set, the finish gates, and verified evidence for a change, without
//! a human in the loop. It is **dev-only** (never shipped), **environment-independent** (stdio,
//! model-neutral, no OpenCode coupling), and **never edits source** — it is the AI's brain and
//! verifier, not its hands.
//!
//! This crate is the M0/M1 scaffold. See `plan/photocraft-dev-mcp-plan.md` in the authoring repo.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod probe;

mod archive;
mod artifact;
mod checks;
mod context;
mod diagnostics;
mod dormant;
mod events;
mod git;
mod graph;
mod impact;
mod jobs;
mod ledger;
mod maintenance;
mod scenario;
mod server;
mod toolset;
mod xtask;

pub use server::{Forge, serve_stdio};
