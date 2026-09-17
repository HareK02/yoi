# Generated TypeScript inventory

`server-api` is the Rust source authority for the checked-in Server HTTP DTO projections in this directory.
Regenerate them from the repository root with:

```sh
cargo run -q -p server-api --features typescript --example generate_legacy_typescript > web/workspace/src/lib/generated/legacy-server-api.ts
cargo run -q -p server-api --features typescript --example generate_workdir_api_types > web/workspace/src/lib/generated/workdir-api.ts
cargo run -q -p server-api --features typescript --example generate_worker_launch_api_types > web/workspace/src/lib/generated/worker-launch-api.ts
cargo run -q -p server-api --features typescript --example generate_companion_api_types > web/workspace/src/lib/generated/companion-api.ts
cargo run -q -p server-api --features typescript --example generate_memory_api_types > web/workspace/src/lib/generated/memory-api.ts
cargo run -q -p server-api --features typescript --example generate_skill_api_types > web/workspace/src/lib/generated/skill-api.ts
cargo run -q -p server-api --features typescript --example generate_auth_api_types > web/workspace/src/lib/generated/auth-api.ts
cargo run -q -p server-api --features typescript --example generate_repository_access_types > web/workspace/src/lib/generated/repository-access-api.ts
```

`legacy-server-api.ts` is the bounded aggregate projection retained during the OpenAPI migration. It is not the final OpenAPI-derived Frontend artifact and must not become a second contract authority. The other files are narrower projections generated from the same Rust source.

`ticket-api.ts` and `protocol.ts` are owned by the separate `ticket` and `protocol` authorities respectively.
