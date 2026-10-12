# workspace-server

Backend that manages persistent Workspace resources and authorization and provides an HTTP API. Coordinates Ticket, Objective, and Worker assignments, Workdir links, and Backend Jobs. Runtime and the controller own Worker execution state and input admission; this crate implements neither the product CLI nor the execution engine.

## Design entry points

- [`server`] — HTTP admission and resource operation coordination
- [`store`] / [`authority`] — SQLite persistence and Workspace resource projections
- [`hosts`] / [`worker_source`] — Runtime delegation and request source verification
- [`backend_job`] — Immutable input, attempt, and structured result boundaries
- [`workspace_catalog`] — Workspace creation and replay contracts
