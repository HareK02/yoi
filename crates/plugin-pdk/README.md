# plugin-pdk

Guest-side Rust helpers for the versioned Component Model Tool and Service contracts. [`ToolContext`] decodes only non-secret invocation data; [`ToolOutput`] rejects oversized data and separates structured output from presentation text. [`export_component_tool!`] adapts generated WIT bindings and preserves the result's error arm.

Host authorization, schema validation, effect receipts and execution belong to the Host, not this crate. [`export_component_service!`] adapts authored lifecycle/event handlers to the separate exact Service world without a guest task loop. The PDK does not import WASI, implement a Host, or provide Workdir/WebSocket stubs. Old v1 packages require an explicit v2 rebuild rather than a compatibility adapter.
