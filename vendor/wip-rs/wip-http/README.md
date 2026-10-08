# wip-http

Framework-independent WIP over HTTP v1 endpoint, route, JSON codec, and response-classification support.

The crate maps the three WIP Core interactions to fixed HTTP routes and strict UTF-8 JSON representations. It constructs and validates `http` request and response values but deliberately performs no network I/O and does not define TLS, authentication, proxy, or connection policy.

```toml
[dependencies]
wip-http = "0.1.0"
```

```rust
use wip_http::wip_protocol::ObserveRequest;
use wip_http::{Endpoint, Limits, encode_observe_request};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = Endpoint::parse("https://example.test/wip")?;
    let limits = Limits::new(64 * 1024, 1024 * 1024, 64)?;
    let request = encode_observe_request(
        &endpoint,
        &ObserveRequest { path: "/".into(), depth: 0 },
        limits,
    )?;
    assert_eq!(request.uri(), "https://example.test/wip/v1/observe");
    Ok(())
}
```

## Interface references and scope identity

Every interface reference is encoded as an exact JSON object with required
`scope` and `name` strings, for example `{"scope":"/github","name":"issue.read"}`.
This applies to `Object.interfaces` elements, fetch request/response `interface`,
and call `interface.reference`. Scope must be a canonical absolute Worldspace
path and name must be nonempty. Legacy opaque strings, short-form strings,
relative scopes, missing fields, and extra members are rejected. Unicode, `::`,
and `#` in names remain literal data; comparison uses decoded field values,
independent of JSON member order and escapes. HTTP endpoint prefixes do not
change Worldspace scope.

Fetch responses and call interface targets additionally carry optional
`scope_ref`, an opaque string copied from the scope Object's `ref` observation.
It is not base64 and is not part of interface identity. Absent values are omitted;
null and nonstring values are rejected. Descriptor-independent call metadata
validates reference shape, but leaves scope ancestry and identity preconditions
to the Host's `InterfaceMismatch` check, preserving protocol error precedence.
Invalid success responses remain client-local `InvalidResponse` errors; protocol
error envelopes and transport failures retain their existing classification.

See the [API documentation](https://docs.rs/wip-http) for codec and failure-boundary details.

Licensed under either of Apache-2.0 or MIT at your option.
