//! Forge — the PhotoCraft development MCP.
//!
//! Forge is an **AI-first development control plane** for PhotoCraft: it gives any MCP-capable AI
//! the must-change-together touch set, the finish gates, and verified evidence for a change, without
//! a human in the loop. It is **dev-only** (never shipped), **environment-independent** (stdio,
//! model-neutral, no OpenCode coupling), and **never edits source** — it is the AI's brain and
//! verifier, not its hands.
//!
//! This crate is the M0/M1 scaffold. See `plan/photocraft-dev-mcp-plan.md` in the authoring repo.

pub mod probe;

mod server;

pub use server::{Forge, serve_stdio};
