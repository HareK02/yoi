# content-history-store

Persists immutable content and append-only editing history on the caller's SQLite transaction.
The pure canonical model belongs to `content-history`; this crate does not evaluate configuration,
edit Tickets, authenticate users, or replace their existing CAS contracts.

[`ScopedStore`] is a trusted Host adapter, not an untrusted SQL service. Its caller implements
[`Authority`] using existing current operation authorization for the exact [`ScopeKey`]. Every
operation rechecks authority before reading stored metadata. A hash is never a capability: even
preparation requires complete verified bytes, and public reads require reachability from an
adopted history in that same workspace and target. Physical deduplication does not grant access.

[`SCHEMA`] is the domain's schema contribution. The eventual adopting Server domain must install
it through its existing ordered migration and fresh-schema path, not through a second startup
migration runner. This foundation does not change the production configuration writer. The
adapter accepts the same `rusqlite::Transaction` used by the Server. The caller prepares complete
objects, validates one candidate snapshot, calls [`ScopedStore::adopt`] with the observed history
head, and updates source/projection/business records before committing that **same** transaction.
An error must roll back the caller's transaction; a successful adapter call alone is not successful
business adoption. Each individual mutating method also uses a savepoint, so catching its error
cannot retain its partial changes. SQLite writer acquisition and BUSY/retry policy remain caller-owned.
No automatic retries or external effects are hidden here.

Normal edits retain all adopted ancestors and their full snapshots; [`ScopedStore::collect_unadopted`]
only removes preparations not used by any adopted history. Object rows cannot be overwritten
through the adapter. A supplied hash/bytes mismatch, a differing existing row, a missing child,
wrong child kind or unsupported canonical format fails rather than publishing an incomplete head.
Stored bytes are verified on every read, not reserialized under a newer version.

[`ScopedStore::erase`] requires separate explicit authorization, tombstones the exact target
permanently and removes its ownership references. Old hashes cannot recreate that target's data.
Other targets' retained or prepared references keep shared physical objects alive. A newly created
business object needs a new target identity; erasure is not an edit or a rewind. This is logical
store erasure, not secure disk overwrite or revocation of already exported copies/backups. The
caller owns retention decisions, backup policy and authenticated editor/time facts. Restoring an
old content root is a new adoption with the **current** history parent, never resetting head to an
old history. Existing Ticket `content_digest`, Ticket/post identities and business events remain
separate contracts owned by their domain.
