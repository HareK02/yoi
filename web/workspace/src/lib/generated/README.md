# Generated frontend contracts

Checked-in files in this directory are generated API contracts. Do not edit them by hand.

## Repository list/detail/create API

`repository-api.ts` is the only TypeScript declaration authority for the Repository list, detail, and create request/response/error wire types. It is generated from the schema closure of the five Repository operations in the canonical `openapi/server-api.json` artifact. The generated header pins the in-repository generator name/version/options, canonical input path and digest, and output path.

Regenerate or verify it from the repository root:

```sh
cargo run -q -p server-api --example generate_repository_openapi_types
cargo run -q -p server-api --example generate_repository_openapi_types -- --check
```

The generator fails closed on unsupported or lossy OpenAPI constructs, including ambiguous nullable unions, external references, unsupported enum literals, and integer ranges that cannot be represented safely by JavaScript numbers. The legacy TypeScript generator imports the two shared Repository source projection types it still needs and no longer declares the migrated Repository list/detail types.

## Other generated contracts

The remaining files retain their generator command in their header. `legacy-server-api.ts` is transitional: new bounded domain slices should use a focused contract artifact instead of adding another API surface to it.
