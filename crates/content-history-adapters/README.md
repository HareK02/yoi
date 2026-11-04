# content-history-adapters

Contract examples mapping configuration sources and Ticket compound documents onto the independent content-history model. These adapters prepare immutable candidates and restore complete content; they do not save, authorize, evaluate, adopt, or migrate business data.

- [`settings`] preserves virtual paths, full UTF-8 sources, and explicit entry metadata.
- [`ticket`] separates document content and original authorship from business state and edit history.
- [`Snapshot`] supplies a candidate root and objects for caller-owned store preparation. A successful preparation is not business adoption; the caller must validate and atomically adopt source, projection, and history head in its own transaction.

Readers must be scoped to an authorized object/root by the store. Knowing a hash is not authorization. Missing or erased content must fail restoration rather than be reconstructed from another scope. Retention, explicit erasure, expected-history-head CAS, and new authenticated editor history on restoration belong to the store and caller, not these examples.
