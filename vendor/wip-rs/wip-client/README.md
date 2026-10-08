# wip-client

Presentation-independent stateful runtime for Web Interface Protocol (WIP) clients.

`wip-client` manages endpoint- and security-context-isolated Known Space, object and Interface Descriptor observations, bounded request ownership, supersession, out-of-order response suppression, validator recovery, and operation call contexts. It produces and consumes framework-neutral `wip-http` messages but performs no network I/O and contains no UI types.

```toml
[dependencies]
wip-client = "0.1.0"
```

Applications provide the HTTP adapter and presentation layer, send each prepared request exactly as owned by the runtime, then return the corresponding response for completion.

## Interface identity and scope lifetime

Interface acquisition and call APIs use `wip_protocol::InterfaceReference { scope, name }` directly. Within an endpoint/security-context session, the complete scoped pair is the cache key and the Object membership key: equal names in different scopes remain distinct. A scope is a canonical absolute Worldspace path and must be an ancestor of, or equal to, the call target path. Root scope (`/`) and self scope are valid; clients must not rewrite either into a different reference.

`InterfaceObservation::scope_ref` is optional opaque metadata identifying the scope Object, not a third reference component, a path, or an Interface Descriptor validator. Its absence does not prevent acquisition or calls. Calls copy the exact observed reference, optional `scope_ref`, and optional Interface Descriptor validator into `InterfaceTarget`; the target Object validator is copied separately. The `CallContext` fixes those observations, the descriptor, and the operation declaration before dispatch.

A descriptor without an Interface Descriptor validator is immutable only within the lifetime of its scope Object, not forever at that path. Descriptors with validators are also bound to that scope lifetime. When accepted observations establish that a known scope binding has changed or been lost, dependent cached interface state is invalidated and cannot authorize a new call until explicitly reacquired. A superseded interface response must not restore an observation from the old lifetime. A previously prepared call retains its original context rather than silently adopting a new scope identity or descriptor.

Scope binding evidence is shared across Interface names at the same scope path: a new binding learned by fetching one name retires older descriptors for the other names too. Separately observed Object refs also provide lifetime evidence even when a descriptor's `scope_ref` was omitted. This corroboration is private bounded cache state, not an invented identity, and never fills in missing call metadata. Same-lifetime descriptor validation still applies; a detected ref change or loss retires the old lifetime instead.

Object deletion, entry-edge removal, and local cache eviction are different events. A direct observe `NotFound` confirms unavailability at the requested scope path and removes cached interfaces scoped to that path; it does not guarantee identity-wide Object deletion or unavailability through every alias. Removing one entry edge likewise does not prove that the Object was deleted or invalidate every remaining alias. Cache eviction is local loss of retained knowledge, not evidence of Host deletion. Losing a known binding at an interface's scope path nevertheless ends safe reuse of descriptors dependent on that binding, even if the same Object remains reachable elsewhere. Optional `scope_ref` can provide identity evidence, but clients do not fabricate it when it is absent.

Without Object `ref` or `scope_ref` identity evidence, a replacement at the same scope path can be indistinguishable from the previous Object. The runtime cannot detect every ref-less replacement from path and descriptor contents alone; absence of detectable change is not proof that the scope lifetime continued.

## Explicit recovery and operation outcomes

`ensure_observed` and `ensure_interface` avoid duplicate fresh or loading work; they do not automatically refresh retained stale or failed observations. Use the explicit object/children refresh APIs and `prepare_interface` to recover the observations required for a new call. An operation `InterfaceMismatch` invalidates both sides of the call's assumptions: it marks the target Object observation stale and invalidates the selected cached interface observation, so recovery requires reobserving membership and reacquiring the descriptor. Reobserving an Object and reacquiring its interface are retrievals, not a retry of an operation.

The runtime never automatically retries an operation. Mark a call dispatched when handing it to the transport and report completion or transport failure with its original `RequestId`. A timeout, disconnect, or invalid response after dispatch can leave the outcome unknown: execution or commit cannot be ruled out. Refreshing stale state does not resolve that uncertainty and must not automatically resubmit the call; any new operation is an explicit application decision.

See the [API documentation](https://docs.rs/wip-client) for session, acquisition, cache, and call APIs.

Licensed under either of Apache-2.0 or MIT at your option.
