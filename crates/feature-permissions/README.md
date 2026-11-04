# Feature permissions

This crate owns payload-free declarations, assignment DTOs, catalog projections,
explicit children resolution and operation-key delegation limits. It does not
adopt policy, migrate old grants, authorize a real operation, or choose Worker
permissions. See [`PermissionHost`] for the trusted registration boundary and
[`types`] for the shared text/storage/management/Web representation.

The Host authenticates a Workspace and a builtin or trusted Plugin identity
before registering it. It retains [`FeatureHandle`] and supplies only the bound
[`NamespaceHandle`] to that Feature's declaration/query adapter. Guest input is
[`NamespaceDeclaration`], never an owner string or a handle. Constructing another
Host with the same textual Workspace identity does not create interchangeable
handles or results. A namespace conveys ownership, not permission to execute.

Children are graph edges, not dotted-prefix wildcards. A key can have multiple
segments and explicit children; a shared set collects Feature contributions using
the same references. Installing a declaration validates its candidate graph
atomically. Duplicate definitions, unknown references and cycles reject the update
without changing the prior catalog. Duplicate assignment paths are deduplicated,
with distinct provenance paths retained in deterministic order. Invalid assignment
references deny the whole assignment with diagnostics. Disabled contributions are
omitted independently; removing a Feature retires its handles and withdraws only
its declarations. Dangling references deny rather than recovering an old result.

Every change invalidates prior [`ResolvedPermissions`]. The Host re-resolves the
current assignment before use. A resolution retains its actual catalog snapshot;
query rejects it unless that snapshot is still the Host's current catalog. There
is no independently ordered freshness counter. Unset,
explicit empty, exclusion and revocation never select a default. Exclusions expand
through the same children graph, then win over every grant path. No snapshot is a
policy store: consumers must read current policy and authenticated subject/Job
context at their actual operation boundary.

A [`LimitedGrant`] freezes the expanded keys actually passed at issuance. Later
set or children additions can extend a set-holding parent, not that grant. Child
execution intersects the fresh parent execution, fresh parent delegation ceiling
and frozen grant. A [`JobGrant`] is one further intersection bound to the Host's
Job identity; it never substitutes for Subject policy, attempt/writer/input/expiry
checks. Revoked grant objects cannot be made active again. Operation-key checks do
not replace provider path, recursive, symlink, network or domain constraints.

A text/storage/management assignment uses an explicit discriminant, for example:

```json
{
  "grants": [
    {"kind":"set", "value":"companion"},
    {"kind":"namespace", "value":{"namespace":"ticket", "keys":["ticket.hoge.fuga"]}}
  ],
  "exclusions": [],
  "revoked": false
}
```

The same Serde types validate dynamic inputs; unknown fields (including payloads)
are rejected. [`typescript`] generates the Web contract. Management projections are
serializable observations, not credentials: deserialization cannot manufacture a
runtime resolution, namespace handle or delegation grant. The consumer owns
policy persistence and its route/subject authentication, not this DTO layer.
