# api-macros

Defines transport-independent API metadata from public traits. The same declarations drive optional HTTP adapters and OpenAPI generation, while callers retain authentication, authorization, and application state.

## Main entry points

- [`api`] — API declaration and validation
- [`ApiContract`], [`Operation`], and [`WebSocketOperation`] — typed operation contracts
- [`HttpError`] — opt-in public error encoding, with unchanged JSON defaults
- `axum`, `reqwest`, and `openapi` — adapters and document generation enabled by their respective features
- `http` — projected error-contract response conversion and bounded reception, enabled by `error-contract` without enabling a framework
