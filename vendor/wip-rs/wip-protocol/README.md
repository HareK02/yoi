# wip-protocol

Transport-independent logical types and validation for the Web Interface Protocol (WIP).

This crate defines the canonical Object and Interface Descriptor models, the three Core interaction request and response types, schema-neutral operation `Value`s, validators, protocol errors, and structural validation. It does not define an HTTP mapping, application DTOs, persistence, or network I/O.

```toml
[dependencies]
wip-protocol = "0.1.0"
```

Interface identity is an exact `InterfaceReference { scope, name }` pair.
`scope` is a canonical absolute Worldspace path and `name` is nonempty Unicode.
Use `validate()` for shape, `validate_for_path()` for ancestor-or-self placement,
and Object/interaction validation at response boundaries. Fetching needs no
target Object path. Fetch responses and interface call targets carry optional
opaque string `scope_ref` metadata, not a third identity field. Descriptors do
not embed identity. See the repository's [migration guide](../../MIGRATION.md)
for the breaking Rust and HTTP wire changes and deferred consumer semantics.

See the [API documentation](https://docs.rs/wip-protocol) for the complete model and validation rules.

Licensed under either of Apache-2.0 or MIT at your option.
