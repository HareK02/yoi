# Runtime request proof rejection diagnostics

`runtime_request_proof_rejected` (`yoi::auth`, WARN) is an internal operational
record. HTTP responses are unchanged: 401 with no proof details (repository
routes retain their existing generic API error envelope). It does **not** establish
why the reported same-machine intermittent 401 occurred. No reproduction or
causal measurement of that incident has been obtained.

## Verification and correlation

- `request_id`: Server-generated UUID identifying this rejection event. It is not
  a caller-supplied ID or a response header, and is not a cross-service trace ID.
- `claimed_runtime_id_hash`, `claimed_worker_id_hash`, `claimed_token_id_hash`:
  lowercase 64-character hex SHA-256 of
  `b"yoi-runtime-proof-diagnostic-id-v1\0" + id.as_bytes()`.
  `worker_source::diagnostic_id_hash` exposes this correlation function. Hash a
  known Runtime/Worker ID to find its failures; token ID hashes correlate replay
  attempts without logging the proof or JTI itself. These are fingerprints, not
  encryption: low-entropy IDs can be guessed. Do not place secrets in IDs.
- All claims-derived fields are prefixed `claimed_`, even after signature checks.
  `proof_verification` describes check progress, **not an authorized principal**:
  - `unverified`: decoding may have succeeded, but signature verification has not.
  - `signature_verified`: signature passed; request binding or time failed.
  - `request_verified`: signature, request bindings and time passed, but replay,
    membership or authority failed.
- When decoding fails, there are no claims fingerprints. Do not infer a Runtime
  from a malformed proof. No raw claims, proof, key, header, body or raw error is
  logged. The body digest and claim mismatch values are also deliberately absent.
- Method, query-free path, and requested workspace are request metadata, not
  authenticated claims. They are bounded to 32, 512 and 128 UTF-8 bytes, with
  control characters removed. Truncated paths are not unique identifiers. Query
  names and values are never included. Paths are operational metadata, not a
  place to put secrets.

## Reasons and their limits

Decoding distinguishes `invalid_format`, `invalid_encoding`, and
`malformed_claims`; no parser error text is included. Signature checks distinguish
`invalid_runtime_public_key` from `invalid_signature` (which cannot tell whether
an attacker, wrong key, corruption, or configuration mismatch caused the failure).

Request binding reports the first mismatch, in the existing check order:
`issuer_mismatch`, `audience_mismatch`, `workspace_mismatch`, `worker_mismatch`,
`permission_mismatch`, `method_mismatch`, `path_mismatch`, `body_digest_mismatch`.
The server selects the trusted key by the unverified issuer and expects that
same issuer/worker; issuer and worker mismatches can be diagnosed by the lower
verifier but are not independently detectable by the current server caller.
These diagnostics do not add new identity checks or change authorization.

`backend_audience_unavailable` denotes missing Backend public URL configuration.
`runtime_trust_missing_or_revoked` deliberately covers both absence and revocation,
as the existing store lookup/filter does not preserve that distinction.
`replay` means JTI consumption returned false. `worker_catalog_membership` means
neither registry membership nor an active create reservation was found.

Store failures identify their check stage without exposing raw errors:
`runtime_trust_authority_unavailable`, `replay_authority_unavailable`,
`worker_catalog_authority_unavailable`, `worker_reservation_authority_unavailable`,
`worker_singleton_authority_denied`. The last can represent either an owner
conflict or an authority failure: the store currently returns a shared error,
so the diagnostic does not guess which. `verification_time_out_of_range` means
that conversion to the JTI consumption timestamp failed; it is not proof expiry.
`invalid` is a fallback for lower verifier errors not classified for this path.

## Time contract and T-728 coordination

This change preserves strict integer-second checks, inclusive bounds, and
verification order: signature, request binding, then `iat > now`, then `exp < now`.
No skew tolerance is added. If both time predicates fail, future issuance wins.

- `issued_in_future`: `time_delta_seconds = iat - now` (positive).
- `expired`: `time_delta_seconds = now - exp` (positive).
- Only these reasons include `issued_at_unix`, `expires_at_unix`,
  `verified_at_unix` and `time_tolerance_seconds` (currently zero).
- Deltas use `i64::abs_diff`, preserving the full positive `u64` range without
  overflow. All times come from the actual signature-verified rejection and
  the exact clock sample used for that decision, not a second log-time sample.

T-728 owns any change to tolerance/validity policy. If it changes these checks,
the error time data, logged effective tolerance, and boundary tests must change
together. Do not label future issuance as expiry or independently recompute the
rejection using a different clock sample. This is instrumentation for the next
incident, not evidence that the original incident has been resolved.
