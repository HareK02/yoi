# feature-storage

Owns physical SQLite identity and lifecycle for trusted Host repositories.
Features own domain schema, authorization and transactions; storage identity is not a grant.
Workers and models do not receive SQL connections.

[`ScopedFeatureStorage::provision`] is fresh-only;
[`ScopedFeatureStorage::adopt_existing`] requires explicit Host authority and
persisted domain attribution. [`ScopedFeatureStorage::open_existing`] consumes
an expected Workspace/Feature/physical identity and never creates a missing DB.
The lazy [`ScopedFeatureStorage::open`] requests initial Host provisioning only
when no ownership record exists; an unbound existing file requires adoption,
and a missing previously bound file is an error, not a new incarnation.

[`FeatureDatabase`] operations retain the verified connection and shared
cross-process lifecycle lease through consumer commit/rollback.
[`DatabaseGuard`] extends the short lease over a writer-release decision without
retaining a SQLite transaction. Long Job waits retain only [`DatabaseIdentity`];
resumption must reacquire storage and current domain authorization.

Lock order is scope lifecycle file, manager state, connection, SQLite transaction.
The Host must protect the root, owner catalog and stable lockfiles against native
code bypassing the owner. A successful same-root lock does not prove arbitrary
external writers, a previous Host, or a different filesystem participate.
Same-path replacement is checked by physical file identity as well as DB identity;
old receipt bytes alone cannot authorize a copied file. Restore records a new
physical lifecycle and retains original provenance. A durable erasure fence lives
outside the removed scope and cannot be cleared by restoring an older snapshot.

`restore` recovers missing scope files using the same owner's retained source
receipts, never by calling `delete` first. `restore_from` requires the Host to
select the source owner explicitly for a cross-root copy. Both check completed
snapshot provenance and the source erasure fence through publication; a fresh
root is not a way to forget that fence. Cross-root locks are acquired in canonical
root order, source shared and target exclusive; same-root recovery takes one
exclusive lock. A restore retires, but does not remove, the previous owner
receipt. Only the new physical identity is current, including after exact retry.
A snapshot without matching source-owner evidence requires explicit, authorized
legacy adoption with domain attribution/shape validation instead of blind restore.
These checks cover owner-managed operations, not deliberate destruction of the
owner catalog/fences or arbitrary Host code inventing authority evidence.

Adoption identity and domain migrations share a SQLite transaction. A separate
owner receipt binds the stable request, target, authority evidence and inode.
If the DB commits but completion or its response is lost, the same request
reconciles the same physical DB. An unfinished reservation is not an adoption
receipt; a different target or request cannot reuse it. Failure after a fresh file
is created but before its reservation commits leaves an unowned file requiring
explicit Host reconciliation, never discovery-based issuance.

[`FeatureRegistration`] uses the existing ordered migration owner; a Feature can
validate its finite published receipt/shape families without rewriting original
migration identities or treating physical acceptance as domain-policy adoption.
[`FeatureStorage::open_host_catalog`] connects the Host catalog to the same
identity/lifecycle owner; [`configure_connection`] owns shared SQLite durability policy.
