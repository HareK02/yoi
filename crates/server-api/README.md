# server-api

HTTP contract shared by Workspace Server and Rust clients. Owns public DTOs, API declarations, and generated client/server adapters. workspace-server determines persistent resource authorization and state transitions; worker-runtime determines Runtime execution state.

## Contract entry points

- [`ServerApi`] — HTTP operations and transport metadata
- [`ServerRequestContext`] — Request context passed to Server authentication adapters
- [`RequestActor`] — Human identity and authentication method
- [`WorkerRestoreRequest`] / [`WorkerRestoreResult`] — Observation-based Restore intent and result
