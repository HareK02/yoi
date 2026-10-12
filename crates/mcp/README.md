# mcp

`mcp` implements local stdio MCP server startup, JSON-RPC communication, discovery, and shutdown. The `worker` Feature owns decisions to register MCP Tools, resources, and prompts for models; this crate handles communication and process lifetime.

## Main entry points

- [`stdio`](src/stdio.rs): Server launch configuration resolution, bounded responses, diagnostics, and shutdown.
