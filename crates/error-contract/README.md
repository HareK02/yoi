# Error contract

This crate owns typed cause/context preservation, current-context disclosure interfaces,
bounded codecs and RFC 9457 primitives. It does not own operation outcomes, authorization,
retry, cleanup, persistence, HTTP servers, logging sinks or production domain mappings.

[FailureFacts](crate::FailureFacts) retains the concrete domain cause. A domain derives
identity from its variant and implements [ProjectFor](crate::ProjectFor) per audience,
selecting typed wire fields and common metadata using the boundary's current authenticated
context and resource visibility. Only [project_failure](crate::project_failure) produces an
encodable audience marker. Sources and Display are never automatically serialized.

[codec] distinguishes validated, unsupported and corrupt input. A received value must be
reprojected; opaque unsupported payload cannot be blindly forwarded to another audience.
[problem] uses API-boundary classification, not inner status forwarding. The `schema` module
(with `json-schema`) exposes the same DTOs to existing schema generators. The `test_support`
module (with `test-support`) provides two independent sample domains and shared wire fixtures for
consumer contract tests, not production authorization adapters.
