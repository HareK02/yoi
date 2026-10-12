# flow

`flow` compiles DCDL state graphs into typed definitions and evaluates transitions against pinned definitions and verification results. Callers own Workspace source persistence, Worker Session writes, and internal verifier execution. This crate implements neither role-specific work loops nor state-entry side effects.

## Main entry points

- [`compile_flow_source`](src/definition.rs): Source evaluation and graph validation.
- [`FlowSelector`](src/selector.rs) / [`ResolvedFlowSource`](src/selector.rs): Explicit source identity and immutable snapshots.
- [`FlowRuntimeState`](src/coordinator.rs): Instances, transition attempts, and ordered events.
- [`builtin_flow_source`](src/builtin.rs): Read-only catalog of embedded sources.
