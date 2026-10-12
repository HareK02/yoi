# config-source-wasm

WASM adapter exposing `config-source` snapshot evaluation and editing support to JavaScript. It handles browser-local edit candidates without changing Backend save permissions or persistent state.

## Main entry points

- [`set_snapshot`] and [`apply_changes`] — update local edit candidates
- [`evaluate_snapshot`] and [`evaluate_current`] — evaluation through the shared implementation
- [`analyze_snapshot`] and [`complete_current`] — diagnostics and completion
