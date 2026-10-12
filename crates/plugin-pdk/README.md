# plugin-pdk

Guest-side JSON/WIT helpers for offline Component Model packages. Without depending on host/runtime crates, it converts Tool and Service inputs/outputs and bounds Tool output size. Using the PDK or validating a package does not authorize installation or execution in a Worker.

## Main entry points

- [`run_json_tool`] and [`export_component_tool!`] — typed Tool input/output boundary
- [`ToolOutput`] and [`ToolError`] — bounded output and errors
- [`ServiceOutput`] and [`export_plugin_instance!`] — instance output contracts
