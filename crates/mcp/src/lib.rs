//! Model Context Protocol client foundations.
//!
//! This crate intentionally only owns protocol/lifecycle plumbing. It does not
//! register MCP tools, resources, or prompts into Yoi's model-visible tool
//! surface.

pub mod stdio;
