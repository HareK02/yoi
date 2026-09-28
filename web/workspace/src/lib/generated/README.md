# Generated frontend contracts

Checked-in files in this directory are generated API contracts. Do not edit them by hand.

## Repository list/detail/create API

`repository-api.ts` is the only TypeScript declaration authority for the Repository list, detail, and create request/response/error wire types. It is generated from the schema closure of the five Repository operations in the canonical `openapi/server-api.json` artifact. The generated header pins the in-repository generator name/version/options, canonical input path and digest, and output path.

Regenerate or verify it from the repository root:

```sh
cargo run -q -p server-api --example generate_repository_openapi_types
cargo run -q -p server-api --example generate_repository_openapi_types -- --check
```

The generator fails closed on unsupported or lossy OpenAPI constructs, including ambiguous nullable unions, external references, semantic `$ref` siblings, unimplemented ordinary schema keywords, unsupported enum literals/formats, and integer ranges that cannot be represented safely by JavaScript numbers. Its one explicit format mapping is canonical `uint64` to a nonnegative JavaScript safe `number`; the runtime parser enforces the same range. The legacy TypeScript generator imports the two shared Repository source projection types it still needs and no longer declares the migrated Repository list/detail types.

## Workspace subscription / Worker protocol

`protocol.ts` and `protocol-validator.ts` are generated together from the Serde/`schemars` closure of `protocol::subscription::SubscriptionFrame`. Rust DTOs and validation constants in `crates/protocol` remain the sole wire authority. The generator normalizes Serde defaults to the actual serialized required/nullable shape, tightens every supported Rust integer format to its JavaScript-safe wire range, and fails on an unknown integer format. The validator closes every generated object shape, rejects unknown tagged variants and unsafe integers, applies Rust-owned aggregate bounds to object keys as well as values, and mirrors the Browser-relevant `SubscriptionFrame::validate()` selector/payload checks. Rust-serialized compatibility fixtures are embedded for Browser regression tests. The Workspace multiplexer additionally checks live event payloads against its locally retained selector.

Inbound Browser frames only are validated: Browser-to-Server methods remain compile-time typed and are decoded plus semantically validated by the Rust server. Invalid inbound frames close the shared socket with a fixed, non-reflective diagnostic; the existing reconnect path clears routing state and requests fresh snapshots. A valid `subscription_closed` frame continues to isolate and resubscribe only that subscription.

Regenerate or verify both artifacts from the repository root:

```sh
cargo run -q -p protocol --example generate_typescript --features typescript
cargo run -q -p protocol --example generate_typescript --features typescript -- --check
```

The `protocol` crate tests also compare both checked-in files byte-for-byte with fresh generator output.

## Other generated contracts

The remaining files retain their generator command in their header. `legacy-server-api.ts` is transitional: new bounded domain slices should use a focused contract artifact instead of adding another API surface to it.
