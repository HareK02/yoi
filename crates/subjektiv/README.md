# subjektiv

Shared Host domain implementation for per-Subject Memory, change history, candidate dispositions, derived surfaces, and Session activity Journals.
`feature-storage` manages physical SQLite storage; the calling Host handles authentication and execution authority.
Journal storage does not generate prose, launch Jobs, or fetch Session history.

## Main entry points

- [`SubjektivStore`]: scope-bound records and transactions
- [`MemoryRecord`], [`SurfaceSnapshot`]: change history and derived context
- [`api`]: shared Host operations and bounded responses
- [`job`]: consolidation selection rules and result validation
- [`journal`]: individually committed activity operations, source coverage, retry receipts, and bounded reading
