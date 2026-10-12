# subjektiv

Shared Host domain implementation for per-Subject Memory, change history, candidate dispositions, and derived surfaces.
`feature-storage` manages physical SQLite storage; the calling Host handles authentication and execution authority.

## Main entry points

- [`SubjektivStore`]: scope-bound records and transactions
- [`MemoryRecord`], [`SurfaceSnapshot`]: change history and derived context
- [`api`]: shared Host operations and bounded responses
- [`job`]: consolidation selection rules and result validation
