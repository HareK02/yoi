# Service Component fixture

`ticket-service-v2.component.wasm` is the authored Rust/PDK guest from
`crates/plugin-runtime/tests/service-guest`, not a synthetic Component.
Its manifest remains the source manifest there (Service world 2.2, Host imports
2.0/2.1). The workspace-server integration suite installs and validates the
package, fetches the pinned artifact, and executes the Component through the
production Feature Service task/owner adapters.

Regenerate with wasm32-unknown-unknown std and `lld` available:

```sh
cargo test -p yoi-plugin-runtime --test service_component
cp target/plugin-service-fixture/service.component.wasm \
  crates/workspace-server/tests/fixtures/ticket-service-v2.component.wasm
```

The first command builds the guest source using `--locked --offline --release`,
then uses `wit_component::ComponentEncoder` to validate/encode it. No E2E feature
flag, external service, or process-global environment mutation is needed for the
workspace-server suite; its only HTTPS peer is the isolated shared TLS fixture.
